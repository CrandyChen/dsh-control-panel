//! 卸载：枚举「安装目录 + DSH 用户数据目录(~/.dsh)」形成清单，
//! 用户勾选确认后逐条删除。删除前强制校验路径来源与安全性。
//!
//! 清单统计（stat）使用目录枚举缓存（DirEntry）快速遍历，并支持：
//! - 进度上报：通过 Channel 定时推送 `scan` 步骤的 Output 行；
//! - 取消：扫描期间检查 AtomicBool 标志，置位立即中止返回「已取消」。
//! 删除前的路径校验只做存在性检查（list_preview_paths），不再二次全量统计。

use std::cell::Cell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::AppHandle;

use crate::config;
use crate::detect::dsh_home;
use crate::error::AppError;
use crate::logging::Logger;
use crate::process::{PipelineEvent};

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UninstallEntry {
    pub id: String,
    pub name: String,
    pub path: String,
    pub kind: String,
    pub size: u64,
    pub items: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UninstallPreview {
    pub entries: Vec<UninstallEntry>,
    pub install_dir: Option<String>,
    pub dsh_home: String,
}

/// 进度上报上下文（卸载预览统计用）：label 用于进度文案，channel 推送输出。
struct Progress<'a> {
    label: &'a str,
    channel: &'a Channel<PipelineEvent>,
    last_emit: Cell<Instant>,
}

/// 递归统计：不跟随符号链接；限制深度与条目数防止卡死。
/// 用 `DirEntry::file_type()` / `metadata()` 读取目录枚举缓存（Windows 上来自
/// FindFirst/FindNext 的结果，无需逐条打开句柄），比逐条 `symlink_metadata`
/// 快数倍。`cancel` 置位时立即返回 Err（卸载预览取消用）。
fn walk(
    p: &Path,
    size: &mut u64,
    items: &mut u64,
    depth: u32,
    cancel: Option<&AtomicBool>,
    prog: Option<&Progress>,
) -> Result<(), String> {
    if depth > 48 || *items > 200_000 {
        return Ok(());
    }
    *items += 1; // 目录本身计 1 项（与旧实现口径一致）。
    let rd = match std::fs::read_dir(p) {
        Ok(rd) => rd,
        Err(_) => return Ok(()), // 目录不可读：跳过，不中断。
    };
    for entry in rd.flatten() {
        if let Some(c) = cancel {
            if c.load(Ordering::Relaxed) {
                return Err(crate::i18n::t("uninstall.scan.cancelled"));
            }
        }
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if ft.is_symlink() {
            // 符号链接计 1 项，取自身长度（不跟随目标，避免环）。
            *items += 1;
            if let Ok(md) = std::fs::symlink_metadata(entry.path()) {
                *size += md.len();
            }
        } else if ft.is_file() {
            *items += 1;
            if let Ok(md) = entry.metadata() {
                *size += md.len();
            }
        } else if ft.is_dir() {
            walk(&entry.path(), size, items, depth + 1, cancel, prog)?;
        }
        if let Some(pr) = prog {
            let now = Instant::now();
            if now.duration_since(pr.last_emit.get()) >= std::time::Duration::from_millis(250) {
                pr.last_emit.set(now);
                let _ = pr.channel.send(PipelineEvent::Output {
                    step: "scan".into(),
                    stream: "stdout".into(),
                    line: crate::i18n::t_fmt(
                        "uninstall.scan.progress",
                        &[pr.label, &items.to_string(), &format_bytes(*size)],
                    ),
                });
            }
        }
    }
    Ok(())
}

/// 统计路径大小与条目数（不跟随符号链接；限制深度与条目数防止卡死）。
/// 纯函数版本（无进度 / 取消），供测试与内部复用。
#[allow(dead_code)]
pub fn stat_path(p: &Path) -> (u64, u64) {
    let mut size = 0;
    let mut items = 0;
    let _ = walk(p, &mut size, &mut items, 0, None, None);
    (size, items)
}

/// 带进度上报与取消的统计（卸载预览用）。取消时返回 Err（i18n 文案）。
pub fn stat_path_progress(
    p: &Path,
    label: &str,
    channel: &Channel<PipelineEvent>,
    cancel: &AtomicBool,
) -> Result<(u64, u64), String> {
    let mut size = 0;
    let mut items = 0;
    let prog = Progress {
        label,
        channel,
        last_emit: Cell::new(Instant::now()),
    };
    walk(p, &mut size, &mut items, 0, Some(cancel), Some(&prog))?;
    Ok((size, items))
}

/// 字节数人类可读格式（与前端 formatSize 口径一致）。
pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let units = ["KB", "MB", "GB", "TB"];
    let mut v = bytes as f64 / 1024.0;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", units[i])
}

/// 内核条目的展示名称（注明安装方式与版本，按界面语言输出）。
pub fn kernel_display_name(mode: &str, version: &str) -> String {
    let key = if mode == "source" {
        "kernel.display.source"
    } else {
        "kernel.display.prebuilt"
    };
    crate::i18n::t_fmt(key, &[version])
}

/// 生成卸载预览清单条目：每个已安装内核版本目录各一项 + DSH 用户数据目录一项。
/// 目录不存在时对应条目省略；返回值纯函数，便于测试。
#[allow(dead_code)]
pub fn build_preview_entries(
    kernels: &[config::KernelInstall],
    dsh_home: &str,
) -> Vec<UninstallEntry> {
    let mut entries = Vec::new();
    for k in kernels {
        let p = Path::new(&k.install_dir);
        if p.exists() {
            let (size, items) = stat_path(p);
            entries.push(UninstallEntry {
                id: k.id.clone(),
                name: kernel_display_name(&k.mode, &k.version),
                path: k.install_dir.clone(),
                kind: "directory".into(),
                size,
                items,
            });
        }
    }
    let hp = Path::new(dsh_home);
    if hp.exists() {
        let (size, items) = stat_path(hp);
        entries.push(UninstallEntry {
            id: "dsh-home".into(),
            name: format!("DSH 用户数据目录（{dsh_home}）"),
            path: dsh_home.to_string(),
            kind: "directory".into(),
            size,
            items,
        });
    }
    entries
}

/// 快速列出卸载候选路径（仅存在性检查，不做统计），供删除前校验清单来源，
/// 避免删除时对超大目录做二次全量统计。
pub fn list_preview_paths(kernels: &[config::KernelInstall], dsh_home: &str) -> Vec<String> {
    let mut v = Vec::new();
    for k in kernels {
        if Path::new(&k.install_dir).exists() {
            v.push(k.install_dir.clone());
        }
    }
    if Path::new(dsh_home).exists() {
        v.push(dsh_home.to_string());
    }
    v
}

/// 带进度上报与取消的清单构建（卸载预览用）。取消时返回 Err。
fn build_preview_entries_progress(
    kernels: &[config::KernelInstall],
    dsh_home: &str,
    channel: &Channel<PipelineEvent>,
    cancel: &AtomicBool,
) -> Result<Vec<UninstallEntry>, String> {
    let mut entries = Vec::new();

    for k in kernels {
        let p = Path::new(&k.install_dir);
        if p.exists() {
            let label = format!("{}（{}）", kernel_display_name(&k.mode, &k.version), k.install_dir);
            let (size, items) = stat_path_progress(p, &label, channel, cancel)?;
            entries.push(UninstallEntry {
                id: k.id.clone(),
                name: kernel_display_name(&k.mode, &k.version),
                path: k.install_dir.clone(),
                kind: "directory".into(),
                size,
                items,
            });
        }
    }

    let hp = Path::new(dsh_home);
    if hp.exists() {
        let label = format!("DSH 用户数据（{dsh_home}）");
        let (size, items) = stat_path_progress(hp, &label, channel, cancel)?;
        entries.push(UninstallEntry {
            id: "dsh-home".into(),
            name: format!("DSH 用户数据目录（{dsh_home}）"),
            path: dsh_home.to_string(),
            kind: "directory".into(),
            size,
            items,
        });
    }

    Ok(entries)
}

/// 生成卸载预览清单（带进度上报与取消）：所有已安装内核版本目录 + DSH 用户数据目录。
/// 取消时返回 Err（前端按取消静默处理）。
pub fn build_preview(
    app: &AppHandle,
    channel: &Channel<PipelineEvent>,
    cancel: &AtomicBool,
) -> Result<UninstallPreview, String> {
    let cfg = config::load_config(app);
    let home = dsh_home();
    let entries =
        build_preview_entries_progress(&cfg.installed_kernels, &home, channel, cancel)?;
    Ok(UninstallPreview {
        entries,
        install_dir: cfg.install_dir.clone(),
        dsh_home: home,
    })
}

/// 安全护栏：拒绝删除盘符根、Windows 目录、用户主目录本身等危险路径。
pub fn is_forbidden_path(p: &Path) -> bool {
    let s = p.to_string_lossy().to_lowercase();
    if s.len() <= 3 {
        return true;
    }
    for d in b'a'..=b'z' {
        let root = format!("{}:\\", d as char);
        if s == root || s.starts_with(&format!("{}windows", root)) {
            return true;
        }
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        if !home.is_empty() && s == home.to_lowercase() {
            return true;
        }
    }
    false
}

/// 执行卸载：校验每个路径都在预览清单内且安全，然后逐条删除。
/// 删除内核版本目录后从注册表移除对应记录，并重算活动内核。
pub fn uninstall(
    app: &AppHandle,
    selected: Vec<String>,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    // 先停止 web 服务，避免文件占用。
    let _ = crate::web::stop_web(app);

    let cfg = config::load_config(app);
    let home = dsh_home();
    // 仅校验路径是否在预览清单内（存在性检查，不做全量统计）。
    let allowed: HashSet<String> =
        list_preview_paths(&cfg.installed_kernels, &home).into_iter().collect();

    for sel in &selected {
        if !allowed.contains(sel) {
            return Err(AppError::NotInPreview(sel.clone()).friendly());
        }
        if is_forbidden_path(Path::new(sel)) {
            return Err(AppError::InvalidPath(sel.clone()).friendly());
        }
    }

    for sel in &selected {
        let p = PathBuf::from(sel);
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| sel.clone());
        let _ = channel.send(PipelineEvent::StepStarted {
            id: "delete".into(),
            title: crate::i18n::t_fmt("step.delete", &[&name]),
        });
        logger.info(&crate::i18n::t_fmt("log.uninstall_delete", &[sel]));

        let is_dir = std::fs::symlink_metadata(&p)
            .map(|md| md.is_dir())
            .unwrap_or(false);
        let result = if is_dir {
            std::fs::remove_dir_all(&p)
        } else {
            std::fs::remove_file(&p)
        };
        match result {
            Ok(()) => {
                let _ = channel.send(PipelineEvent::StepFinished {
                    id: "delete".into(),
                    exit_code: 0,
                });
            }
            Err(e) => {
                let msg = crate::i18n::t_fmt("log.uninstall_delete_failed", &[sel, &e.to_string()]);
                let _ = channel.send(PipelineEvent::Error { message: msg.clone() });
                logger.error(&msg);
                return Err(msg);
            }
        }
    }

    let _ = channel.send(PipelineEvent::Finished { ok: true });

    let mut cfg = config::load_config(app);
    // 从注册表移除所有被删除的内核目录记录。
    let mut removed_kernel_dir: Option<String> = None;
    for k in cfg.installed_kernels.clone() {
        if selected.iter().any(|s| s == &k.install_dir) {
            if removed_kernel_dir.is_none() {
                removed_kernel_dir = Some(k.install_dir.clone());
            }
            config::remove_kernel(&mut cfg, &k.id);
        }
    }
    // 若卸载的是内核目录（且非共享数据目录），清空活动镜像字段。
    if let Some(dir) = removed_kernel_dir {
        if cfg.install_dir.as_deref() == Some(dir.as_str()) {
            cfg.install_dir = None;
            cfg.installed_version = None;
            cfg.installed_commit = None;
        }
        cfg.update_available = false;
        cfg.latest_commit = None;
        cfg.latest_subject = None;
    }
    // 重算活动内核：仍有其它内核时切到最近启动/最新预构建；否则保持空。
    if let Some(active) = config::resolve_active_kernel(&cfg) {
        config::set_active_kernel(&mut cfg, &active.id);
    }
    config::save_config(app, &cfg).map_err(|e| e)?;

    logger.info(&crate::i18n::t("log.uninstall_done"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forbidden_paths_are_rejected() {
        assert!(is_forbidden_path(Path::new("C:\\")));
        assert!(is_forbidden_path(Path::new("C:\\Windows")));
        assert!(is_forbidden_path(Path::new("D:\\")));
        assert!(is_forbidden_path(Path::new("C:\\")));
    }

    #[test]
    fn normal_paths_are_allowed() {
        assert!(!is_forbidden_path(Path::new("D:\\dev\\deepseek-harness")));
        assert!(!is_forbidden_path(Path::new("C:\\Users\\crandy\\.dsh\\profiles")));
    }

    #[test]
    fn stat_path_counts_files() {
        let tmp = std::env::temp_dir().join(format!("dsh-stat-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("a/b")).unwrap();
        std::fs::write(tmp.join("a/one.txt"), "hello").unwrap();
        std::fs::write(tmp.join("a/b/two.txt"), "world!").unwrap();
        let (size, items) = stat_path(&tmp);
        assert_eq!(size, 11);
        assert_eq!(items, 5); // 根 + a + one.txt + b + two.txt
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn walk_aborts_when_cancel_flag_set() {
        // 取消标志置位时，统计立即返回「已取消」（卸载预览取消路径）。
        let tmp = std::env::temp_dir().join(format!("dsh-walk-cancel-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("a/b")).unwrap();
        std::fs::write(tmp.join("a/one.txt"), "x").unwrap();
        let cancel = AtomicBool::new(true);
        let mut size = 0;
        let mut items = 0;
        let err = walk(&tmp, &mut size, &mut items, 0, Some(&cancel), None).unwrap_err();
        assert_eq!(err, crate::i18n::t("uninstall.scan.cancelled"));
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn format_bytes_is_human_readable() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn list_preview_paths_checks_existence_only() {
        let base = std::env::temp_dir().join(format!("dsh-paths-{}", std::process::id()));
        let a = base.join("a");
        std::fs::create_dir_all(&a).unwrap();
        let kernels = [config::KernelInstall {
            id: "prebuilt-1".into(),
            mode: "prebuilt".into(),
            version: "1.0.0".into(),
            install_dir: a.to_string_lossy().to_string(),
            commit: None,
            installed_at: String::new(),
        }];
        let v = list_preview_paths(&kernels, "Z:\\nope-xyz");
        assert_eq!(v, vec![a.to_string_lossy().to_string()]);
        // 不存在的路径省略。
        let none = list_preview_paths(&[], "Z:\\no-dsh-xyz");
        assert!(none.is_empty());
        std::fs::remove_dir_all(&base).ok();
    }

    // ---------- 卸载清单颗粒度（每个内核版本目录一项 + 用户数据目录一项） ----------

    #[test]
    fn preview_lists_kernels_and_dsh_home_as_entries() {
        let base = std::env::temp_dir().join(format!("dsh-uninstall-{}", std::process::id()));
        let install = base.join("dsh-0.1.2-alpha.2");
        let home = base.join(".dsh");
        std::fs::create_dir_all(install.join("node_modules/.bin")).unwrap();
        std::fs::create_dir_all(home.join("profiles/web")).unwrap();
        std::fs::write(install.join("node_modules/.bin/dsh.cmd"), "@echo off").unwrap();
        std::fs::write(home.join("profiles/web/package.json"), "{}").unwrap();

        let kernels = [config::KernelInstall {
            id: "prebuilt-0.1.2-alpha.2".into(),
            mode: "prebuilt".into(),
            version: "0.1.2-alpha.2".into(),
            install_dir: install.to_string_lossy().to_string(),
            commit: None,
            installed_at: String::new(),
        }];
        let entries = build_preview_entries(&kernels, &home.to_string_lossy());
        assert_eq!(entries.len(), 2, "应只有内核版本目录与用户数据目录两项");
        assert_eq!(entries[0].id, "prebuilt-0.1.2-alpha.2");
        assert!(entries[0].path.contains("dsh-0.1.2-alpha.2"));
        assert_eq!(entries[1].id, "dsh-home");
        assert!(entries[1].path.ends_with(".dsh"));

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn preview_omits_missing_paths() {
        let base = std::env::temp_dir().join(format!("dsh-uninstall2-{}", std::process::id()));
        let install = base.join("dsh-0.1.0");
        std::fs::create_dir_all(&install).unwrap();

        // 用户数据目录不存在：只剩内核版本目录一项。
        let kernels = [config::KernelInstall {
            id: "prebuilt-0.1.0".into(),
            mode: "prebuilt".into(),
            version: "0.1.0".into(),
            install_dir: install.to_string_lossy().to_string(),
            commit: None,
            installed_at: String::new(),
        }];
        let entries = build_preview_entries(&kernels, &base.join(".dsh").to_string_lossy());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "prebuilt-0.1.0");

        // 两者都不存在：空清单。
        let none = build_preview_entries(&[], "Z:\\no-dsh-xyz");
        assert!(none.is_empty());

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn kernel_display_name_notes_mode_and_version() {
        crate::i18n::set_lang(crate::i18n::Lang::Zh);
        assert_eq!(
            kernel_display_name("prebuilt", "0.1.2-alpha.2"),
            "预构建内核安装 - 0.1.2-alpha.2"
        );
        assert_eq!(kernel_display_name("source", "0.1.1-rc.2"), "源码安装 - 0.1.1-rc.2");
    }
}
