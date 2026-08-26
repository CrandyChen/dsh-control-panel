//! 便携运行时（node + pnpm）按需下载。
//!
//! 目标：发布包不再内嵌 node/pnpm，改为在安装 / 启动需要时下载到程序运行目录下的
//! `runtime/` 子目录。下载与解压复用 `prebuilt.rs` 的 PowerShell + 进度机制；
//! 步骤 id 固定为 `runtime`（前端以次要小字展示，避免与内核下载主进度混淆）。
//!
//! 下载策略：**国内优先，国外备用**，每个依赖最多重试 3 轮；仍失败则明确报错并中止
//! 调用方的安装 / 更新 / 修复 / 启动流程，**不降级使用本机全局 node/pnpm**。
//!
//! pnpm 兼容两种形态：
//! - **JS 包（国内 npm 镜像 tgz）**：`node_modules/pnpm/bin/pnpm.cjs`，由内置 node 运行；
//! - **独立二进制（GitHub standalone zip）**：`runtime/pnpm.exe`。
//! 两种都统一生成可执行的 `runtime/pnpm.cmd`。

use std::path::{Path, PathBuf};

use tauri::ipc::Channel;

use crate::config;
use crate::logging::Logger;
use crate::process::PipelineEvent;

/// 运行时版本（与 scripts/package-portable.mjs 保持一致）。
pub const NODE_VER: &str = "24.19.0";
pub const PNPM_VER: &str = "11.23.0";

/// Node.js 下载地址（国内 npm 镜像优先，国外 nodejs.org 备用）。
pub fn node_domestic_url() -> String {
    format!("https://registry.npmmirror.com/-/binary/node/v{NODE_VER}/node-v{NODE_VER}-win-x64.zip")
}
pub fn node_overseas_url() -> String {
    format!("https://nodejs.org/dist/v{NODE_VER}/node-v{NODE_VER}-win-x64.zip")
}

/// pnpm 下载地址：国内为 npm 镜像 tgz（JS 包），国外为 GitHub standalone zip（独立二进制）。
pub fn pnpm_domestic_tgz_url() -> String {
    format!("https://registry.npmmirror.com/pnpm/-/pnpm-{PNPM_VER}.tgz")
}
pub fn pnpm_overseas_zip_url() -> String {
    format!("https://github.com/pnpm/pnpm/releases/download/v{PNPM_VER}/pnpm-win32-x64.zip")
}
pub fn pnpm_overseas_zip_alt_url() -> String {
    format!("https://github.com/pnpm/pnpm/releases/download/v{PNPM_VER}/pnpm-win-x64.zip")
}

/// 每个依赖（node / pnpm）的重试轮数（每轮依次尝试国内 → 国外）。
const RETRY_ROUNDS: usize = 3;

/// node 就绪：`runtime/node.exe` 存在。
pub fn node_ready() -> bool {
    config::runtime_dir().join("node.exe").is_file()
}

/// pnpm 就绪：`runtime/pnpm.cmd` 或 `runtime/pnpm.exe` 或 `node_modules/pnpm/bin/pnpm.cjs` 任一存在。
pub fn pnpm_ready() -> bool {
    let rt = config::runtime_dir();
    rt.join("pnpm.cmd").is_file()
        || rt.join("pnpm.exe").is_file()
        || rt.join("node_modules").join("pnpm").join("bin").join("pnpm.cjs").is_file()
}

/// 运行环境是否已就绪（node + pnpm 都必须可用）。
pub fn ready() -> bool {
    node_ready() && pnpm_ready()
}

// ─────────────────────────────── 基础下载/解压 ───────────────────────────────

fn download_asset(url: &str, dest: &Path, channel: &Channel<PipelineEvent>) -> Result<(), String> {
    crate::prebuilt::download_asset(url, dest, 0, channel, "runtime").map_err(|e| e.friendly())
}

/// 解压运行环境（node/pnpm）到目标目录，step 固定为 `runtime-ex`（与下载相位区分，便于前端显示解压进度）。
fn extract(zip: &Path, dest: &Path, channel: &Channel<PipelineEvent>) -> Result<(), String> {
    crate::prebuilt::extract_zip_with_progress(zip, dest, channel, "runtime-ex").map_err(|e| e.friendly())
}

/// 将 src 下内容平铺复制到 dst（若 src 恰好只有一个包装目录则取其内部）。
fn flatten_move(src: &Path, dst: &Path) -> Result<(), String> {
    let entries = std::fs::read_dir(src).map_err(|e| e.to_string())?;
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut files: Vec<PathBuf> = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            dirs.push(p);
        } else {
            files.push(p);
        }
    }
    let root = if dirs.len() == 1 && files.is_empty() {
        dirs[0].clone()
    } else {
        src.to_path_buf()
    };
    copy_dir_contents(&root, dst)?;
    Ok(())
}

/// 递归复制目录内容到目标目录。
fn copy_dir_contents(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for e in std::fs::read_dir(src).map_err(|e| e.to_string())?.flatten() {
        let from = e.path();
        let to = dst.join(e.file_name());
        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            if to.is_dir() {
                let _ = std::fs::remove_dir_all(&to);
            }
            copy_dir_contents(&from, &to)?;
        } else {
            let _ = std::fs::remove_file(&to);
            std::fs::copy(&from, &to).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// 从解压目录定位 pnpm JS 包的根：优先 `package/`，否则取唯一子目录，否则原目录。
fn locate_package_dir(ex: &Path) -> PathBuf {
    let direct = ex.join("package");
    if direct.is_dir() {
        return direct;
    }
    if let Ok(rd) = std::fs::read_dir(ex) {
        let dirs: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.path())
            .collect();
        if dirs.len() == 1 {
            return dirs[0].clone();
        }
    }
    ex.to_path_buf()
}

/// 生成 runtime/pnpm.cmd（优先 pnpm.exe 独立二进制，其次 node_modules/pnpm JS 包）。
fn make_pnpm_cmd(rt: &Path) -> Result<(), String> {
    let shim = rt.join("pnpm.cmd");
    if rt.join("pnpm.exe").is_file() {
        std::fs::write(&shim, "@echo off\r\n\"%~dp0pnpm.exe\" %*\r\n").map_err(|e| e.to_string())?;
        return Ok(());
    }
    if rt
        .join("node_modules")
        .join("pnpm")
        .join("bin")
        .join("pnpm.cjs")
        .is_file()
    {
        std::fs::write(
            &shim,
            "@echo off\r\n\"%~dp0node.exe\" \"%~dp0node_modules\\pnpm\\bin\\pnpm.cjs\" %*\r\n",
        )
        .map_err(|e| e.to_string())?;
        return Ok(());
    }
    Err("内置运行时缺少可执行的 pnpm".to_string())
}

// ─────────────────────────────── Node.js ───────────────────────────────

fn assemble_node(
    url: &str,
    rt: &Path,
    tmp: &Path,
    channel: &Channel<PipelineEvent>,
) -> Result<(), String> {
    let zip = tmp.join("node-win.zip");
    let ex = tmp.join("node-ex");
    let _ = std::fs::remove_dir_all(&ex);
    download_asset(url, &zip, channel)?;
    extract(&zip, &ex, channel)?;
    flatten_move(&ex, rt)?;
    let _ = std::fs::remove_file(&zip);
    Ok(())
}

fn install_node(rt: &Path, tmp: &Path, channel: &Channel<PipelineEvent>) -> Result<(), String> {
    let urls = [node_domestic_url(), node_overseas_url()];
    for _ in 0..RETRY_ROUNDS {
        for url in &urls {
            if assemble_node(url, rt, tmp, channel).is_ok() {
                return Ok(());
            }
        }
    }
    Err("Node.js 下载失败（已重试 3 轮）".to_string())
}

// ─────────────────────────────── pnpm ───────────────────────────────

/// JS 包形态（npm 镜像 tgz）：解包到 `runtime/node_modules/pnpm` 并生成 node 转发 shim。
fn assemble_pnpm_tgz(
    url: &str,
    rt: &Path,
    tmp: &Path,
    channel: &Channel<PipelineEvent>,
) -> Result<(), String> {
    let zip = tmp.join("pnpm.tgz");
    let ex = tmp.join("pnpm-ex");
    let _ = std::fs::remove_dir_all(&ex);
    download_asset(url, &zip, channel)?;
    extract(&zip, &ex, channel)?;
    let src = locate_package_dir(&ex);
    let dst = rt.join("node_modules").join("pnpm");
    copy_dir_contents(&src, &dst)?;
    let _ = std::fs::remove_file(&zip);
    make_pnpm_cmd(rt)
}

/// 独立二进制形态（GitHub standalone zip）：解压出 pnpm.exe 并生成转发 shim。
fn assemble_pnpm_zip(
    url: &str,
    alt: &str,
    rt: &Path,
    tmp: &Path,
    channel: &Channel<PipelineEvent>,
) -> Result<(), String> {
    let zip = tmp.join("pnpm.zip");
    let ex = tmp.join("pnpm-ex");
    let _ = std::fs::remove_dir_all(&ex);
    let mut last_err: Option<String> = None;
    for u in [url, alt] {
        if download_asset(u, &zip, channel).is_ok() {
            if extract(&zip, &ex, channel).is_ok() {
                if flatten_move(&ex, rt).is_ok() {
                    let _ = std::fs::remove_file(&zip);
                    return make_pnpm_cmd(rt);
                }
                last_err = Some("pnpm 解压后移动失败".to_string());
            } else {
                last_err = Some("pnpm 解压失败".to_string());
            }
        } else {
            last_err = Some(format!("无法下载 pnpm：{u}"));
        }
        let _ = std::fs::remove_file(&zip);
    }
    Err(last_err.unwrap_or_else(|| "无法下载 pnpm standalone".to_string()))
}

fn install_pnpm(rt: &Path, tmp: &Path, channel: &Channel<PipelineEvent>) -> Result<(), String> {
    for _ in 0..RETRY_ROUNDS {
        // 国内 JS 包优先，国外独立二进制备用。
        if assemble_pnpm_tgz(&pnpm_domestic_tgz_url(), rt, tmp, channel).is_ok() {
            return Ok(());
        }
        if assemble_pnpm_zip(
            &pnpm_overseas_zip_url(),
            &pnpm_overseas_zip_alt_url(),
            rt,
            tmp,
            channel,
        )
        .is_ok()
        {
            return Ok(());
        }
    }
    Err("pnpm 下载失败（已重试 3 轮）".to_string())
}

// ─────────────────────────────── 校验与入口 ───────────────────────────────

/// 校验 runtime 就绪：node.exe 与 pnpm 可执行文件必须存在。
fn verify_runtime(runtime_dir: &Path) -> Result<(), String> {
    if !runtime_dir.join("node.exe").is_file() {
        return Err("内置运行时缺少 node.exe（运行环境解压失败）".to_string());
    }
    if !(runtime_dir.join("pnpm.cmd").is_file() || runtime_dir.join("pnpm.exe").is_file()) {
        return Err("内置运行时缺少可执行的 pnpm（运行环境解压失败）".to_string());
    }
    Ok(())
}

/// 确保便携运行环境就绪（node + pnpm 均已就绪则直接返回；否则按国内/国外源下载）。
/// 失败：返回本地化错误并推送 PipelineEvent::Error；**不降级使用本机全局 node/pnpm**。
pub fn ensure_runtime(channel: &Channel<PipelineEvent>, logger: &Logger) -> Result<(), String> {
    let result = run_ensure_runtime(channel, logger);
    // 无论成功或失败，都通知前端运行环境流程已结束，以隐藏运行环境进度条。
    let _ = channel.send(PipelineEvent::RuntimeDone);
    result
}

fn run_ensure_runtime(channel: &Channel<PipelineEvent>, logger: &Logger) -> Result<(), String> {
    if ready() {
        return Ok(());
    }
    logger.info(&crate::i18n::t("log.runtime_start"));
    let rt = config::runtime_dir();
    let tmp = std::env::temp_dir().join("dsh-runtime");
    let _ = std::fs::create_dir_all(&tmp);

    let result = (|| -> Result<(), String> {
        if !node_ready() {
            install_node(&rt, &tmp, channel)?;
        }
        if !pnpm_ready() {
            install_pnpm(&rt, &tmp, channel)?;
        }
        make_pnpm_cmd(&rt)?;
        verify_runtime(&rt)?;
        Ok(())
    })();

    let _ = std::fs::remove_dir_all(&tmp);
    match result {
        Ok(()) => {
            logger.info(&crate::i18n::t("log.runtime_done"));
            Ok(())
        }
        Err(e) => {
            let msg = crate::i18n::t_fmt("runtime.download_failed", &[&e]);
            let _ = channel.send(PipelineEvent::Error { message: msg.clone() });
            logger.error(&msg);
            Err(msg)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("dsh-runtime-{name}-{}", std::process::id()))
    }

    #[test]
    fn flatten_move_unwraps_single_wrapper_dir() {
        let src = tmp("flat-src");
        let dst = tmp("flat-dst");
        let wrap = src.join("node-v24-win-x64");
        std::fs::create_dir_all(&wrap).unwrap();
        std::fs::write(wrap.join("node.exe"), "bin").unwrap();
        std::fs::write(wrap.join("npm.cmd"), "npm").unwrap();
        flatten_move(&src, &dst).unwrap();
        assert!(dst.join("node.exe").is_file());
        assert!(dst.join("npm.cmd").is_file());
        std::fs::remove_dir_all(&src).ok();
        std::fs::remove_dir_all(&dst).ok();
    }

    #[test]
    fn verify_runtime_checks_node_and_pnpm() {
        let dir = tmp("verify");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(verify_runtime(&dir).is_err()); // 缺 node.exe
        std::fs::write(dir.join("node.exe"), "x").unwrap();
        assert!(verify_runtime(&dir).is_err()); // 缺 pnpm
        std::fs::write(dir.join("pnpm.exe"), "x").unwrap();
        assert!(verify_runtime(&dir).is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn make_pnpm_cmd_generates_node_shim_when_cjs_present() {
        let dir = tmp("cjs");
        let bin = dir.join("node_modules").join("pnpm").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("pnpm.cjs"), "// pnpm").unwrap();
        make_pnpm_cmd(&dir).unwrap();
        let content = std::fs::read_to_string(dir.join("pnpm.cmd")).unwrap();
        assert!(content.contains("pnpm.cjs"));
        assert!(content.contains("node.exe"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn make_pnpm_cmd_generates_exe_shim_when_pnpm_exe_present() {
        let dir = tmp("exe");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pnpm.exe"), "x").unwrap();
        make_pnpm_cmd(&dir).unwrap();
        let content = std::fs::read_to_string(dir.join("pnpm.cmd")).unwrap();
        assert!(content.contains("pnpm.exe"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn locate_package_dir_prefers_package_subdir() {
        let ex = tmp("pkg");
        std::fs::create_dir_all(ex.join("package/bin")).unwrap();
        std::fs::write(ex.join("package/bin/pnpm.cjs"), "// pnpm").unwrap();
        assert_eq!(locate_package_dir(&ex), ex.join("package"));
        std::fs::remove_dir_all(&ex).ok();
    }

    #[test]
    fn make_pnpm_cmd_errors_when_neither_form_present() {
        let dir = tmp("none");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(make_pnpm_cmd(&dir).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
