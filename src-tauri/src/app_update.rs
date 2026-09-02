//! 控制面板自身的更新检测与自动更新。
//!
//! 职责：
//! - 启动 / 手动检测 `https://github.com/CrandyChen/dsh-control-panel/releases` 最新版本；
//! - 检测到新版时**静默后台下载**到 `update/*.zip.part` → 校验（zip 完整 + 含主 exe）→
//!   原子改名 `*.zip`，保证「网络异常 / 程序中途关闭」只留下可判定的完整 / 不完整状态；
//! - **下一次启动**扫描到完整且版本更新的包时，才由前端弹出升级对话框，本模块负责
//!   解压到 `update/stage-<ver>/`、生成英文版 `update/updater.cmd`，随后应用退出、
//!   由 updater.cmd 在等待旧进程退出后备份旧版、原子替换 exe/README/runtime 并启动新版本。
//!
//! 首次下载完成但尚未应用的包**不会**在本轮弹框（设置区提示「下次启动更新」）；
//! 更新只在新版本 > 当前版本时生效；网络失败静默置状态、不弹错误、不影响主流程。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::config;
use crate::logging::Logger;
use crate::process::PipelineEvent;

/// 控制面板发布仓库的最新 release API。
pub const UPDATE_REPO_API: &str =
    "https://api.github.com/repos/CrandyChen/dsh-control-panel/releases/latest";
/// 用于匹配更新包资产名的子串（形如 `DSH-Control-Panel-portable-<ver>-windows-x64.zip`）。
pub const UPDATE_ASSET_SUBSTR: &str = "DSH-Control-Panel-portable-";
pub const UPDATE_ASSET_SUFFIX: &str = "-windows-x64.zip";
/// 更新脚本名（内容为英文，避免中文命令环境带来意外情况）。
pub const UPDATER_CMD: &str = "updater.cmd";

/// 当前控制面板版本（与 Cargo / package.json 一致）。
pub fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// 应用更新状态（序列化为 camelCase，供前端展示/触发升级对话框）。
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum AppUpdateStatus {
    Idle,
    Checking,
    Downloading,
    /// 完整且版本更新的包已就绪（首次下载完成，或启动时已存在）。
    Ready,
    UpToDate,
    Failed,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateState {
    pub status: AppUpdateStatus,
    pub current_version: String,
    pub latest_version: Option<String>,
    pub downloaded: u64,
    pub total: u64,
    pub error: Option<String>,
}

impl Default for AppUpdateState {
    fn default() -> Self {
        Self {
            status: AppUpdateStatus::Idle,
            current_version: app_version(),
            latest_version: None,
            downloaded: 0,
            total: 0,
            error: None,
        }
    }
}

fn set_state(app: &AppHandle, s: AppUpdateState) {
    {
        let state = app.state::<crate::AppState>();
        let mut g = state.app_update.lock().unwrap();
        *g = s.clone();
    }
    let _ = app.emit("app-update-state", &s);
}

fn set_progress(app: &AppHandle, downloaded: u64, total: u64) {
    let snapshot = {
        let state = app.state::<crate::AppState>();
        let mut g = state.app_update.lock().unwrap();
        g.status = AppUpdateStatus::Downloading;
        g.downloaded = downloaded;
        g.total = total;
        g.clone()
    };
    let _ = app.emit("app-update-progress", &snapshot);
    let _ = app.emit("app-update-state", &snapshot);
}

// ─────────────────────────────── 版本解析 ───────────────────────────────

/// 从 tag（可能带 `v` 前缀）解析语义化版本；解析失败返回 None。
fn parse_version(tag: &str) -> Option<semver::Version> {
    semver::Version::parse(tag.trim().trim_start_matches('v')).ok()
}

fn current_semver() -> Option<semver::Version> {
    parse_version(&app_version())
}

/// 从发布包文件名解析版本，形如 `DSH-Control-Panel-portable-<ver>-windows-x64.zip`。
pub fn parse_package_version(file_name: &str) -> Option<String> {
    let rest = file_name.strip_prefix(UPDATE_ASSET_SUBSTR)?;
    let ver = rest.strip_suffix(UPDATE_ASSET_SUFFIX)?;
    if ver.is_empty() {
        None
    } else {
        Some(ver.to_string())
    }
}

/// 应用版本是否更新可用：latest > 当前。
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

// ─────────────────────────────── 检测最新版 ───────────────────────────────

/// 查询最新 release：返回 (tag, 资产下载地址, 资产大小)。失败返回错误（调用方静默处理）。
fn check_latest() -> Result<(String, String, u64), String> {
    let script = r#"$ProgressPreference='SilentlyContinue'
$h=@{'User-Agent'='DSH-Control-Panel'}
$r=Invoke-RestMethod -Uri '@@API@@' -Headers $h
$asset=$r.assets | Where-Object { $_.name -like '@@PATTERN@@' } | Select-Object -First 1
if(-not $asset){ throw 'asset not found' }
Write-Output ($r.tag_name + "`n" + $asset.browser_download_url + "`n" + $asset.size)
"#;
    let script = script
        .replace("@@API@@", UPDATE_REPO_API)
        .replace("@@PATTERN@@", "*DSH-Control-Panel-portable-*-windows-x64.zip");
    let out = crate::prebuilt::powershell_capture(&script).map_err(|e| e.to_string())?;
    let mut lines = out.lines();
    let tag = lines.next().unwrap_or("").trim().to_string();
    let url = lines.next().unwrap_or("").trim().to_string();
    let size = lines
        .next()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    if tag.is_empty() || url.is_empty() {
        return Err("无法解析最新发布（tag/下载地址为空，可能未找到匹配的更新包）".into());
    }
    Ok((tag, url, size))
}

// ─────────────────────────────── 下载（后台、静默、可靠） ───────────────────────

/// 从 url 下载到 dest.part（单个 PowerShell 子进程，无进度；无黑框）。
fn ps_download(url: &str, dest: &Path) -> Result<std::process::Child, String> {
    let script = r#"
[Console]::OutputEncoding=[System.Text.Encoding]::UTF8
$ProgressPreference='SilentlyContinue'
Invoke-WebRequest -Uri '@@URL@@' -OutFile '@@DEST@@' -UseBasicParsing
"#;
    let script = script
        .replace("@@URL@@", &crate::prebuilt::ps_escape(url))
        .replace("@@DEST@@", &crate::prebuilt::ps_escape(&dest.to_string_lossy()));
    crate::process::no_window(
        Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                script.as_str(),
            ])
            .env("PATH", config::augmented_path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .spawn()
    .map_err(|e| e.to_string())
}

fn ps_download_wait(child: &mut std::process::Child) -> Result<(), String> {
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        let mut raw = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut raw);
        }
        let err = crate::prebuilt::clean_ps_error(&raw);
        return Err(if err.is_empty() {
            format!("PowerShell 退出码 {:?}", status.code())
        } else {
            err
        });
    }
    Ok(())
}

/// 下载并轮询进度，更新 `AppState.app_update` 并推送 `app-update-progress`。
fn download_to_part(
    url: &str,
    dest_part: &Path,
    total: u64,
    app: &AppHandle,
) -> Result<(), String> {
    let _ = std::fs::remove_file(dest_part);
    let mut child = ps_download(url, dest_part)?;

    let dest2 = dest_part.to_path_buf();
    let app2 = app.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let poller = std::thread::spawn(move || {
        while !stop2.load(Ordering::Relaxed) {
            let received = std::fs::metadata(&dest2).map(|m| m.len()).unwrap_or(0);
            set_progress(&app2, received, total);
            std::thread::sleep(Duration::from_millis(300));
        }
    });

    let result = ps_download_wait(&mut child);
    stop.store(true, Ordering::Relaxed);
    let _ = poller.join();
    result
}

/// 完整下载：`*.part` → 校验 zip 完整性 + 含主 exe → 原子改名 `*.zip`。
fn download_release(url: &str, total: u64, app: &AppHandle) -> Result<PathBuf, String> {
    let dir = config::update_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = url
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty() && s.ends_with(".zip"))
        .unwrap_or("DSH-Control-Panel-portable-update.zip")
        .to_string();
    let dest_part = dir.join(format!("{name}.part"));
    let dest = dir.join(&name);

    let effective_total = if total == 0 {
        crate::prebuilt::query_content_length(url).unwrap_or(0)
    } else {
        total
    };

    for attempt in 0..DOWNLOAD_MAX_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(DOWNLOAD_RETRY_DELAY_MS * attempt as u64));
        }
        match download_to_part(url, &dest_part, effective_total, app) {
            Ok(()) => {
                if zip_is_valid(&dest_part) {
                    // 原子改名：仅完整包成为 `.zip`。
                    let _ = std::fs::remove_file(&dest);
                    std::fs::rename(&dest_part, &dest).map_err(|e| e.to_string())?;
                    return Ok(dest);
                }
                // 不完整（损坏）包：删除重试。
                let _ = std::fs::remove_file(&dest_part);
            }
            Err(e) => {
                if attempt + 1 >= DOWNLOAD_MAX_ATTEMPTS {
                    return Err(e);
                }
            }
        }
    }
    Err("下载更新包失败（多次重试仍未得到完整文件）".into())
}

const DOWNLOAD_MAX_ATTEMPTS: u32 = 4;
const DOWNLOAD_RETRY_DELAY_MS: u64 = 1500;

/// zip 是否「完整且含主程序 exe」（用中央目录校验；压缩/截断/非 zip 均不通过）。
fn zip_is_valid(zip: &Path) -> bool {
    crate::prebuilt::zip_uncompressed_totals(zip).is_some()
        && crate::prebuilt::zip_has_entry(zip, "dsh-control-panel.exe")
}

// ─────────────────────────────── 就绪包检测 ───────────────────────────────

/// 扫描 `update/`：返回完整且版本 > 当前的最新包（zip 路径 + 目标版本）。
pub fn has_ready_package() -> Result<Option<(PathBuf, String)>, String> {
    scan_ready_in(&config::update_dir())
}

/// 扫描指定目录（供测试）；逻辑与 `has_ready_package` 一致。
pub(crate) fn scan_ready_in(dir: &Path) -> Result<Option<(PathBuf, String)>, String> {
    if !dir.is_dir() {
        return Ok(None);
    }
    let current = current_semver();
    let mut best: Option<(semver::Version, PathBuf, String)> = None;
    for e in std::fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .flatten()
    {
        let p = e.path();
        if p.extension().map(|x| x.eq_ignore_ascii_case("zip")).unwrap_or(false) {
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(ver) = parse_package_version(name) else {
                continue;
            };
            let Some(v) = parse_version(&ver) else { continue };
            if let Some(cur) = &current {
                if v <= *cur {
                    continue;
                }
            }
            if zip_is_valid(&p) {
                if best.as_ref().map(|(bv, _, _)| v > *bv).unwrap_or(true) {
                    best = Some((v, p.clone(), ver));
                }
            }
        }
    }
    Ok(best.map(|(_, p, v)| (p, v)))
}

// ─────────────────────────────── 升级准备（对话框驱动） ───────────────────────

/// 解压就绪包到 stage，并生成英文版 updater.cmd。
/// `channel` 用于向升级对话框推送 Phase / DownloadProgress 进度事件。
/// 返回更新脚本路径（调用方随后应触发 `restart_to_update`）。
pub fn prepare_update(
    zip: &Path,
    target_version: &str,
    channel: &tauri::ipc::Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<PathBuf, String> {
    let dir = config::update_dir();
    let stage = dir.join(format!("stage-{target_version}"));

    let _ = channel.send(PipelineEvent::Phase {
        title: crate::i18n::t("app_update.phase.extract"),
        percent: 5,
    });
    crate::prebuilt::extract_zip_with_progress(zip, &stage, channel, "app-extract")
        .map_err(|e| e.to_string())?;
    let src_root = locate_app_root(&stage)
        .ok_or_else(|| crate::i18n::t("app_update.stage_missing_exe"))?;

    let _ = channel.send(PipelineEvent::Phase {
        title: crate::i18n::t("app_update.phase.script"),
        percent: 90,
    });
    let cmd = write_updater_cmd(zip, &stage, &src_root, target_version)?;
    let stage_s = stage.to_string_lossy().to_string();
    let cmd_s = cmd.to_string_lossy().to_string();
    logger.info(&crate::i18n::t_fmt(
        "log.app_update_ready",
        &[&stage_s, &cmd_s],
    ));
    Ok(cmd)
}

/// 在解压目录下定位含 `DSH-Control-Panel.exe` 的根目录（兼容 zip 顶层带一个包装目录）。
fn locate_app_root(dir: &Path) -> Option<PathBuf> {
    if dir.join("DSH-Control-Panel.exe").is_file() {
        return Some(dir.to_path_buf());
    }
    let rd = std::fs::read_dir(dir).ok()?;
    let subdirs: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .collect();
    if subdirs.len() == 1 && subdirs[0].join("DSH-Control-Panel.exe").is_file() {
        return Some(subdirs[0].clone());
    }
    None
}

/// 生成更新脚本（全英文，避免中文命令环境意外）。内容：等待旧进程退出 → 备份旧版
/// （exe + config.json）→ 原子替换 exe/README/runtime → 清理 → 启动新版 → 自删。
fn write_updater_cmd(
    zip: &Path,
    stage: &Path,
    src_root: &Path,
    target_version: &str,
) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe_dir = exe
        .parent()
        .ok_or_else(|| "无法确定程序目录".to_string())?
        .to_path_buf();
    let config = exe_dir.join("config.json");
    let backup_dir = config::backup_dir();
    std::fs::create_dir_all(&backup_dir).map_err(|e| e.to_string())?;
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let backup = backup_dir.join(format!(
        "DSH-Control-Panel-{}-{ts}.zip",
        app_version(),
    ));
    let pid = std::process::id();

    let exe_s = exe.to_string_lossy().to_string();
    let dir_s = exe_dir.to_string_lossy().to_string();
    let stage_s = stage.to_string_lossy().to_string();
    let src_s = src_root.to_string_lossy().to_string();
    let zip_s = zip.to_string_lossy().to_string();
    let cfg_s = config.to_string_lossy().to_string();
    let backup_s = backup.to_string_lossy().to_string();

    let body = format!(
        r#"@echo off
rem DSH Control Panel self-updater (English only to avoid issues with localized command prompts).
set "PID={pid}"
set "STAGE={stage_s}"
set "SRC={src_s}"
set "APPEXE={exe_s}"
set "APPDIR={dir_s}"
set "ZIP={zip_s}"
set "BACKUP={backup_s}"
set "CFG={cfg_s}"

rem Wait for the old (running) app to exit so file locks are released.
:wait
tasklist /FI "PID eq %PID%" 2>nul | findstr /I "%PID%" >nul
if %errorlevel%==0 (
  timeout /t 1 /nobreak >nul
  goto wait
)

rem Back up the old executable and config.json.
powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "Add-Type -A System.IO.Compression.FileSystem; $p=@(['%APPEXE%']); if(Test-Path '%CFG%'){{$p += '%CFG%'}}; Compress-Archive -Path $p -DestinationPath '%BACKUP%' -Force"

rem Replace the executable atomically: copy to .new, then move over the old.
copy /y /b "%SRC%\DSH-Control-Panel.exe" "%APPEXE%.new" >nul
move /y "%APPEXE%.new" "%APPEXE%" >nul

rem Update README.txt if provided by the package.
if exist "%SRC%\README.txt" copy /y /b "%SRC%\README.txt" "%APPDIR%\README.txt" >nul

rem Sync bundled runtime if provided by the package.
if exist "%SRC%\runtime" xcopy /e /y /i "%SRC%\runtime" "%APPDIR%\runtime" >nul

rem Clean up staging and the downloaded package.
rmdir /s /q "%STAGE%"
if exist "%ZIP%" del /q "%ZIP%"

rem Launch the new version, then remove this script.
start "" "%APPEXE%"
del /q "%~f0"
"#,
        pid = pid,
        stage_s = stage_s,
        src_s = src_s,
        exe_s = exe_s,
        dir_s = dir_s,
        zip_s = zip_s,
        backup_s = backup_s,
        cfg_s = cfg_s,
    );

    let cmd_path = config::update_dir().join(UPDATER_CMD);
    std::fs::write(&cmd_path, body).map_err(|e| e.to_string())?;
    let _ = target_version; // 预留：可写进脚本/文件名用于调试。
    Ok(cmd_path)
}

/// 启动更新脚本（无黑框）并退出当前程序，由脚本完成后自动重启。
pub fn restart_to_update(app: &AppHandle) -> Result<(), String> {
    let cmd_path = config::update_dir().join(UPDATER_CMD);
    if !cmd_path.is_file() {
        return Err(crate::i18n::t("app_update.no_script"));
    }
    // 用 cmd /c 运行脚本；无窗口，等待旧进程退出后替换。
    let cmd_s = cmd_path.to_string_lossy().to_string();
    crate::process::no_window(
        Command::new("cmd.exe")
            .args(["/c", &cmd_s])
            .env("PATH", config::augmented_path()),
    )
    .spawn()
    .map_err(|e| e.to_string())?;
    app.exit(0);
    Ok(())
}

// ─────────────────────────────── 后台工作线程 ───────────────────────────────

/// 后台工作线程（release 由 lib.rs 调用；debug 下仅用于手动检测，故允许未使用）。
#[cfg_attr(debug_assertions, allow(dead_code))]
pub fn spawn_worker(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        cleanup_stale();
        // 1) 已有就绪包：本次启动直接进入升级。
        if let Ok(Some((_zip, ver))) = has_ready_package() {
            set_state(
                &handle,
                AppUpdateState {
                    status: AppUpdateStatus::Ready,
                    latest_version: Some(ver.clone()),
                    ..Default::default()
                },
            );
            let _ = handle.emit("app-update-ready", &ver);
            return;
        }

        // 2) 否则：自动检测并后台下载（静默，失败仅置状态，不打扰用户）。
        let _ = run_download_loop(&handle);
    });
}

/// 手动检测（设置区按钮）：独立线程，复用下载循环；已有就绪包则直接保持 Ready。
pub fn spawn_manual_check(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        if let Ok(Some((_zip, ver))) = has_ready_package() {
            set_state(
                &handle,
                AppUpdateState {
                    status: AppUpdateStatus::Ready,
                    latest_version: Some(ver),
                    ..Default::default()
                },
            );
            return;
        }
        let _ = run_download_loop(&handle);
    });
}

/// 清理 update 目录里的临时残留（未完成下载 `.part`、临时 stage、版本不高于当前的旧包），
/// 避免堆积。当前可应用的更新包（版本 > 当前且校验通过）保留。
#[cfg_attr(debug_assertions, allow(dead_code))]
fn cleanup_stale() {
    let dir = config::update_dir();
    if !dir.is_dir() {
        return;
    }
    let current = current_semver();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if name.ends_with(".part") {
                let _ = std::fs::remove_file(&p);
            } else if name.starts_with("stage-") && p.is_dir() {
                let _ = std::fs::remove_dir_all(&p);
            } else if name.ends_with(".zip") {
                if let Some(ver) = parse_package_version(&name) {
                    if let Some(v) = parse_version(&ver) {
                        // 版本不高于当前 → 已过时，删除；高于当前 → 保留等待升级。
                        if current.as_ref().map(|c| v <= *c).unwrap_or(false) {
                            let _ = std::fs::remove_file(&p);
                        }
                    }
                }
            }
        }
    }
}

/// 检测并静默下载：发现新版本才下载；下载完成置为 Ready（本轮不弹框）。
fn run_download_loop(app: &AppHandle) -> Result<(), String> {
    set_state(
        app,
        AppUpdateState {
            status: AppUpdateStatus::Checking,
            ..Default::default()
        },
    );
    let current = app_version();
    let (tag, url, size) = match check_latest() {
        Ok(x) => x,
        Err(e) => {
            set_state(
                app,
                AppUpdateState {
                    status: AppUpdateStatus::Failed,
                    error: Some(e.clone()),
                    ..Default::default()
                },
            );
            let logger = app.state::<crate::AppState>().logger.clone();
            logger.warn(&crate::i18n::t_fmt("log.app_update_check_fail", &[&e]));
            return Ok(());
        }
    };
    let Some(latest_v) = parse_version(&tag) else {
        return Ok(());
    };
    if !is_newer(&latest_v.to_string(), &current) {
        set_state(
            app,
            AppUpdateState {
                status: AppUpdateStatus::UpToDate,
                latest_version: Some(latest_v.to_string()),
                ..Default::default()
            },
        );
        return Ok(());
    }

    set_state(
        app,
        AppUpdateState {
            status: AppUpdateStatus::Downloading,
            latest_version: Some(latest_v.to_string()),
            ..Default::default()
        },
    );
    let logger = app.state::<crate::AppState>().logger.clone();
    logger.info(&crate::i18n::t_fmt(
        "log.app_update_download_start",
        &[&latest_v.to_string()],
    ));
    match download_release(&url, size, app) {
        Ok(_dest) => {
            set_state(
                app,
                AppUpdateState {
                    status: AppUpdateStatus::Ready,
                    latest_version: Some(latest_v.to_string()),
                    ..Default::default()
                },
            );
            logger.info(&crate::i18n::t_fmt(
                "log.app_update_download_done",
                &[&latest_v.to_string()],
            ));
        }
        Err(e) => {
            set_state(
                app,
                AppUpdateState {
                    status: AppUpdateStatus::Failed,
                    latest_version: Some(latest_v.to_string()),
                    error: Some(e.clone()),
                    ..Default::default()
                },
            );
            logger.warn(&crate::i18n::t_fmt("log.app_update_download_fail", &[&e]));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("dsh-app-update-{name}-{}", std::process::id()))
    }

    #[test]
    fn parse_package_version_extracts_middle() {
        assert_eq!(
            parse_package_version("DSH-Control-Panel-portable-2.4.0-windows-x64.zip").as_deref(),
            Some("2.4.0")
        );
        assert_eq!(
            parse_package_version("DSH-Control-Panel-portable-2.5.0-rc.1-windows-x64.zip").as_deref(),
            Some("2.5.0-rc.1")
        );
        assert_eq!(parse_package_version("other.zip"), None);
        assert_eq!(parse_package_version("DSH-Control-Panel-portable--windows-x64.zip"), None);
    }

    #[test]
    fn is_newer_compares_semver() {
        assert!(is_newer("2.5.0", "2.4.0"));
        assert!(is_newer("v2.5.0", "2.4.0"));
        assert!(!is_newer("2.4.0", "2.4.0"));
        assert!(!is_newer("2.3.0", "2.4.0"));
        assert!(!is_newer("abc", "2.4.0"));
    }

    #[test]
    fn zip_is_valid_rejects_non_zip() {
        let dir = tmp_dir("invalid");
        let p = dir.join("x.zip");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&p, "not a zip").unwrap();
        assert!(!zip_is_valid(&p));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scan_ready_in_ignores_no_dir_and_invalid() {
        let base = tmp_dir("scan");
        // 目录不存在 → None。
        assert_eq!(scan_ready_in(&base.join("nope")).unwrap(), None);
        // 目录存在但无有效包 → None。
        let dir = base.join("update");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("not-a-zip.zip"), "junk").unwrap();
        assert_eq!(scan_ready_in(&dir).unwrap(), None);
        // 版本不高于当前 → None（构造一个高于当前版本的文件名但非 zip 内容，仍 None）。
        std::fs::write(dir.join("DSH-Control-Panel-portable-99.0.0-windows-x64.zip"), "junk")
            .unwrap();
        assert_eq!(scan_ready_in(&dir).unwrap(), None);
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn locate_app_root_finds_direct_or_nested() {
        let dir = tmp_dir("root");
        // 直接位于目录下。
        let direct = dir.join("direct");
        std::fs::create_dir_all(&direct).unwrap();
        std::fs::write(direct.join("DSH-Control-Panel.exe"), "x").unwrap();
        assert_eq!(locate_app_root(&direct).as_deref(), Some(direct.as_path()));
        // zip 顶层带一个包装目录。
        let nested = dir.join("pkg").join("DSH-Control-Panel-portable-2.4.0-windows-x64");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("DSH-Control-Panel.exe"), "x").unwrap();
        assert_eq!(
            locate_app_root(&dir.join("pkg")).as_deref(),
            Some(nested.as_path())
        );
        // 缺失 → None。
        let empty = dir.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(locate_app_root(&empty), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
