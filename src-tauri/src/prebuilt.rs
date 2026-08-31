//! 预构建内核（模式二）管理：查询 GitHub release、下载并解压 Windows 压缩包、
//! 定位 DSH 根目录。
//!
//! 该模式不需 git/pnpm/npm，也不需用户安装任何运行环境：依托内置运行时
//! （`runtime` 目录内的 node/pnpm）与系统 PowerShell（Windows 自带）完成
//! 网络下载与解压。网络查询使用 GitHub REST API（带 UA），资产名固定为
//! `deepseek-harness-pkg-windows.zip`。
//!
//! 下载：后台运行 PowerShell `Invoke-WebRequest`，主线程轮询目标文件大小，
//! 通过 Channel 推送实时进度（字节数 / 总大小 / 速度）。所有 PowerShell 脚本
//! 强制输出 UTF-8，避免中文 Windows（GBK 控制台）下错误文本乱码。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tauri::ipc::Channel;

use crate::config;
use crate::error::AppError;
use crate::process::PipelineEvent;

/// PowerShell 单引号字符串转义（`'` → `''`）。
pub(crate) fn ps_escape(s: &str) -> String {
    s.replace('\'', "''")
}

/// 强制 PowerShell 输出为 UTF-8 的前缀（重定向 stdout/stderr 时避免 GBK 乱码）。
const PS_UTF8_PREFIX: &str = "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8;";

/// 清洗 PowerShell 错误输出：去掉命令名前缀与噪音行，返回首个有效行。
/// 中文 Windows 下 PowerShell 错误文本原为 GBK 字节，脚本内已强制 UTF-8，
/// 此处只负责去掉 "Invoke-WebRequest : " 之类的前缀与超长内容。
pub fn clean_ps_error(raw: &str) -> String {
    let first = raw
        .lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string();
    let prefixes = [
        "Invoke-WebRequest : ",
        "Invoke-RestMethod : ",
        "Expand-Archive : ",
        "Remove-Item : ",
        "Test-Path : ",
    ];
    let mut s = first;
    for p in prefixes {
        if let Some(rest) = s.strip_prefix(p) {
            s = rest.trim().to_string();
            break;
        }
    }
    if s.chars().count() > 300 {
        s = s.chars().take(300).collect::<String>() + "…";
    }
    s
}

/// 运行 PowerShell 并捕获 stdout；失败返回友好错误。
/// 所有脚本统一前置 UTF-8 输出编码（修复中文系统下 stderr 乱码）。
pub(crate) fn powershell_capture(script: &str) -> Result<String, AppError> {
    let full = format!("{PS_UTF8_PREFIX} {script}");
    let out = crate::process::no_window(
        std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                full.as_str(),
            ])
            .env("PATH", config::augmented_path()),
    )
    .output()
    .map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AppError::ProgramNotFound("powershell".into())
        } else {
            AppError::Io(e.to_string())
        }
    })?;
    if !out.status.success() {
        let raw = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let err = clean_ps_error(&raw);
        return Err(AppError::Io(if err.is_empty() {
            format!("PowerShell 退出码 {:?}", out.status.code())
        } else {
            err
        }));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// 最新 release 信息。
#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub tag: String,
    pub url: String,
    /// 资产大小（字节），用于下载进度展示；解析失败时为 0。
    pub size: u64,
}

/// 一个可安装的预构建内核版本（来自 GitHub release 列表）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrebuiltRelease {
    /// 原始 tag。
    pub tag: String,
    /// 归一化版本号（如 0.1.2-alpha.2）。
    pub version: String,
    /// 资产下载地址（deepseek-harness-pkg-windows.zip）。
    pub url: String,
    /// 资产大小（字节）；解析失败为 0。
    pub size: u64,
    /// 是否为 pre-release。
    pub prerelease: bool,
    /// 发布时间（ISO 8601）。
    pub published_at: String,
}

/// 单次解析时的原始 release 记录（与 PowerShell 输出的 JSON 对齐）。
#[derive(Debug, Clone, serde::Deserialize)]
struct RawRelease {
    tag: String,
    prerelease: bool,
    published: String,
    url: String,
    size: u64,
}

/// 查询所有预构建内核发布版本（GitHub REST API，无鉴权），返回可安装的版本列表，
/// 按版本号降序。失败自动重试多次；release list 可能含无资产的历史版本，已过滤。
pub fn list_prebuilt_releases() -> Result<Vec<PrebuiltRelease>, AppError> {
    let mut last_err: Option<AppError> = None;
    for attempt in 0..3u32 {
        match list_prebuilt_releases_once() {
            Ok(r) => return Ok(r),
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(1000 * (attempt as u64 + 1)));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| {
        AppError::PrebuiltDownload("查询预构建内核版本列表失败".into())
    }))
}

/// 单次查询（无重试版本）。
fn list_prebuilt_releases_once() -> Result<Vec<PrebuiltRelease>, AppError> {
    let script = r#"
$ProgressPreference='SilentlyContinue'
$h=@{'User-Agent'='DSH-Control-Panel'}
$r=Invoke-RestMethod -Uri '@@RELEASES@@' -Headers $h
$items=@()
foreach($rel in $r){
  if($rel.draft){ continue }
  $asset=$rel.assets | Where-Object { $_.name -like '*@@ASSET@@*' } | Select-Object -First 1
  if(-not $asset){ continue }
  $items += [pscustomobject]@{
    tag=$rel.tag_name
    prerelease=[bool]$rel.prerelease
    published=[string]$rel.published_at
    url=[string]$asset.browser_download_url
    size=[uint64]$asset.size
  }
}
$items | ConvertTo-Json -Compress
"#;
    let script = script
        .replace("@@RELEASES@@", config::PREBUILT_PKG_RELEASES_API)
        .replace("@@ASSET@@", config::PREBUILT_PKG_ASSET);
    let out = powershell_capture(&script)?;
    if out.trim().is_empty() {
        return Ok(Vec::new());
    }
    let raws: Vec<RawRelease> = serde_json::from_str(&out)
        .map_err(|e| AppError::PrebuiltDownload(format!("解析预构建内核版本列表失败: {e}")))?;
    let mut releases: Vec<PrebuiltRelease> = raws
        .into_iter()
        .filter(|r| !r.tag.trim().is_empty() && !r.url.trim().is_empty())
        .map(|r| {
            let version = crate::version::normalized_tag_version(&r.tag);
            PrebuiltRelease {
                tag: r.tag,
                version,
                url: r.url,
                size: r.size,
                prerelease: r.prerelease,
                published_at: r.published,
            }
        })
        .collect();
    // 按归一化版本（语义化优先）降序排列，供前端默认选最新。
    releases.sort_by(|a, b| {
        let ord = compare_version_strings(&b.version, &a.version);
        if ord != std::cmp::Ordering::Equal {
            ord
        } else {
            b.published_at.cmp(&a.published_at)
        }
    });
    // 归一化后可能重复（如带 build 号 tag 归一化相同），按最新 tag 保留首个。
    releases.dedup_by(|a, b| a.version == b.version);
    Ok(releases)
}

fn compare_version_strings(a: &str, b: &str) -> std::cmp::Ordering {
    match (semver::Version::parse(a), semver::Version::parse(b)) {
        (Ok(va), Ok(vb)) => va.cmp(&vb),
        _ => a.cmp(b),
    }
}

/// 按归一化版本号查找对应 release 的下载信息（用于「修复安装」指定已装版本）。
pub fn release_by_version(version: &str) -> Result<ReleaseInfo, AppError> {
    let list = list_prebuilt_releases()?;
    list.into_iter()
        .find(|r| r.version == version)
        .map(|r| ReleaseInfo {
            tag: r.tag,
            url: r.url,
            size: r.size,
        })
        .ok_or_else(|| {
            AppError::PrebuiltDownload(format!("未找到版本 {version} 的预构建资源"))
        })
}

/// 查询最新 release 的 tag、下载地址与资产大小（GitHub REST API，无鉴权）。
/// GitHub 在国内常抽筋：失败自动重试多次（每次递增等待）。
pub fn latest_release() -> Result<ReleaseInfo, AppError> {
    let mut last_err: Option<AppError> = None;
    for attempt in 0..3u32 {
        match latest_release_once() {
            Ok(r) => return Ok(r),
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(1000 * (attempt as u64 + 1)));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| AppError::PrebuiltDownload("查询最新 release 失败".into())))
}

/// 单次查询（无重试版本）。
fn latest_release_once() -> Result<ReleaseInfo, AppError> {
    let script = r#"
$ProgressPreference='SilentlyContinue'
$h=@{'User-Agent'='DSH-Control-Panel'}
$r=Invoke-RestMethod -Uri '@@API@@' -Headers $h
$asset=$r.assets | Where-Object { $_.name -like '*@@ASSET@@*' } | Select-Object -First 1
if(-not $asset){ throw 'asset not found' }
Write-Output ($r.tag_name + "`n" + $asset.browser_download_url + "`n" + $asset.size)
"#;
    let script = script
        .replace("@@API@@", config::PREBUILT_PKG_API)
        .replace("@@ASSET@@", config::PREBUILT_PKG_ASSET);
    let out = powershell_capture(&script)?;
    let mut lines = out.lines();
    let tag = lines.next().unwrap_or("").trim().to_string();
    let url = lines.next().unwrap_or("").trim().to_string();
    let size = lines
        .next()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    if tag.is_empty() || url.is_empty() {
        return Err(AppError::PrebuiltDownload(
            "无法解析最新 release（tag/下载地址为空，可能未找到匹配的资产）".into(),
        ));
    }
    Ok(ReleaseInfo { tag, url, size })
}

/// 通过 HEAD 请求探测资源 Content-Length（用于显示真实进度）；失败返回 None。
pub fn query_content_length(url: &str) -> Option<u64> {
    let script = r#"
$ProgressPreference='SilentlyContinue'
try {
  $r=Invoke-WebRequest -Uri '@@URL@@' -Method Head -UseBasicParsing
  if($r.Headers['Content-Length']){ Write-Output $r.Headers['Content-Length'] }
} catch { Write-Output '' }
"#;
    let script = script.replace("@@URL@@", &ps_escape(url));
    let out = powershell_capture(&script).ok()?;
    out.trim().parse::<u64>().ok()
}

/// 生成下载用的 PowerShell 命令并启动子进程（stdout/stderr 走管道）。
fn spawn_download(url: &str, dest: &Path) -> Result<std::process::Child, AppError> {
    let script = r#"
[Console]::OutputEncoding=[System.Text.Encoding]::UTF8
$ProgressPreference='SilentlyContinue'
Invoke-WebRequest -Uri '@@URL@@' -OutFile '@@DEST@@' -UseBasicParsing
"#;
    let script = script
        .replace("@@URL@@", &ps_escape(url))
        .replace("@@DEST@@", &ps_escape(&dest.to_string_lossy()));
    crate::process::no_window(
        std::process::Command::new("powershell.exe")
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
    .map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AppError::ProgramNotFound("powershell".into())
        } else {
            AppError::Io(e.to_string())
        }
    })
}

/// 等待下载子进程结束；失败时读取 stderr（已强制 UTF-8）并清洗后返回。
fn wait_download(child: &mut std::process::Child) -> Result<(), AppError> {
    let status = child.wait().map_err(|e| AppError::Io(e.to_string()))?;
    if !status.success() {
        let mut raw = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut raw);
        }
        let err = clean_ps_error(&raw);
        return Err(AppError::PrebuiltDownload(if err.is_empty() {
            format!("PowerShell 退出码 {:?}", status.code())
        } else {
            err
        }));
    }
    Ok(())
}

/// 单次下载尝试：启动 PowerShell 下载，同时轮询目标文件大小并推送进度事件。
/// `total` 为已知资产大小（未知传 0，前端显示为不确定进度）。
fn download_attempt(
    url: &str,
    dest: &Path,
    total: u64,
    channel: &Channel<PipelineEvent>,
    step: &str,
) -> Result<(), AppError> {
    // 清理上次残留的未完成文件，避免轮询到旧大小。
    let _ = std::fs::remove_file(dest);
    let mut child = spawn_download(url, dest)?;

    let dest2 = dest.to_path_buf();
    let ch = channel.clone();
    let step2 = step.to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let poller = std::thread::spawn(move || {
        let mut last = 0u64;
        let mut last_t = std::time::Instant::now();
        let mut speed_bps = 0u64;
        while !stop2.load(Ordering::Relaxed) {
            let received = std::fs::metadata(&dest2).map(|m| m.len()).unwrap_or(0);
            let now = std::time::Instant::now();
            let dt = now.duration_since(last_t).as_secs_f64();
            if dt >= 0.5 {
                let delta = received.saturating_sub(last);
                speed_bps = if dt > 0.0 { (delta as f64 / dt) as u64 } else { 0 };
                last = received;
                last_t = now;
            }
            let _ = ch.send(PipelineEvent::DownloadProgress {
                step: step2.clone(),
                received,
                total,
                speed_bps,
            });
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    });

    let result = wait_download(&mut child);
    stop.store(true, Ordering::Relaxed);
    let _ = poller.join();
    result
}

/// 下载最大尝试次数（GitHub 在国内常抽筋，多试几次）。
const DOWNLOAD_MAX_ATTEMPTS: u32 = 5;
/// 每次失败后的等待（毫秒），随尝试次数递增。
const DOWNLOAD_RETRY_DELAY_BASE_MS: u64 = 1200;

/// 下载指定 url 到 dest 文件，实时推送进度；失败自动重试多次（瞬时网络抖动）。
/// 返回的失败信息已清洗（无命令名前缀 / 乱码）。
pub fn download_asset(
    url: &str,
    dest: &Path,
    total: u64,
    channel: &Channel<PipelineEvent>,
    step: &str,
) -> Result<(), AppError> {
    // 未知大小时先探测 Content-Length，用于显示真实进度（探测失败则不显示确定的百分比）。
    let effective_total = if total == 0 {
        query_content_length(url).unwrap_or(0)
    } else {
        total
    };
    let mut last_err: Option<AppError> = None;
    for attempt in 0..DOWNLOAD_MAX_ATTEMPTS {
        match download_attempt(url, dest, effective_total, channel, step) {
            Ok(()) => return Ok(()),
            Err(e) => {
                let msg = e.to_string();
                last_err = Some(e);
                if attempt + 1 < DOWNLOAD_MAX_ATTEMPTS {
                    // 告警并继续重试（用户日志显示第几次失败）。
                    let note = crate::i18n::t_fmt(
                        "log.prebuilt_download_retry_n",
                        &[&(attempt + 1).to_string(), &msg],
                    );
                    let _ = channel.send(PipelineEvent::Output {
                        step: step.into(),
                        stream: "stderr".into(),
                        line: note.clone(),
                    });
                    std::thread::sleep(std::time::Duration::from_millis(
                        DOWNLOAD_RETRY_DELAY_BASE_MS * (attempt as u64 + 1),
                    ));
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| {
        AppError::PrebuiltDownload("下载预构建内核失败（未知错误）".into())
    }))
}

/// 读取 zip 的未压缩总大小与条目数（经典 zip 中央目录；zip64 / 损坏时返回 None）。
/// 仅用于解压进度展示。
pub fn zip_uncompressed_totals(zip: &Path) -> Option<(u32, u64)> {
    let data = std::fs::read(zip).ok()?;
    if data.len() < 22 {
        return None;
    }
    // 从末尾向前查找 EOCD 签名 PK\x05\x06（0x06054b50）。
    let search_start = data.len().saturating_sub(22 + 65535);
    let mut eocd = None;
    let mut i = data.len() - 22;
    loop {
        if data[i..i + 4] == [0x50, 0x4b, 0x05, 0x06] {
            eocd = Some(i);
            break;
        }
        if i == search_start {
            break;
        }
        i -= 1;
    }
    let eocd = eocd?;
    let total_entries = u16::from_le_bytes([data[eocd + 10], data[eocd + 11]]) as u32;
    let cd_offset = u32::from_le_bytes([
        data[eocd + 16],
        data[eocd + 17],
        data[eocd + 18],
        data[eocd + 19],
    ]) as usize;
    let mut total_size: u64 = 0;
    let mut count: u32 = 0;
    let mut off = cd_offset;
    for _ in 0..total_entries {
        if off + 46 > data.len() || data[off..off + 4] != [0x50, 0x4b, 0x01, 0x02] {
            return None;
        }
        let uncompressed = u32::from_le_bytes([
            data[off + 24],
            data[off + 25],
            data[off + 26],
            data[off + 27],
        ]) as u64;
        let fn_len = u16::from_le_bytes([data[off + 28], data[off + 29]]) as usize;
        let extra_len = u16::from_le_bytes([data[off + 30], data[off + 31]]) as usize;
        let comment_len = u16::from_le_bytes([data[off + 32], data[off + 33]]) as usize;
        total_size += uncompressed;
        count += 1;
        off += 46 + fn_len + extra_len + comment_len;
    }
    Some((count, total_size))
}

/// 递归统计目录下所有文件字节数之和（解压进度用；符号链接按目标大小近似）。
fn dir_total_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            match e.file_type() {
                Ok(ft) if ft.is_dir() => total += dir_total_size(&e.path()),
                _ => {
                    if let Ok(md) = e.metadata() {
                        total += md.len();
                    }
                }
            }
        }
    }
    total
}

/// 执行一次解压子进程，同时轮询 dest 已解压字节数并推送进度事件。
/// 失败时读取 stderr（已强制 UTF-8）并清洗后返回。
fn run_extract_with_progress(
    cmd: &mut std::process::Command,
    dest: &Path,
    total: u64,
    channel: &Channel<PipelineEvent>,
    step: &str,
) -> Result<(), AppError> {
    let mut child = crate::process::no_window(cmd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AppError::ProgramNotFound("解压程序".into())
            } else {
                AppError::Io(e.to_string())
            }
        })?;

    let dest2 = dest.to_path_buf();
    let ch = channel.clone();
    let step2 = step.to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let poller = std::thread::spawn(move || {
        let mut last = 0u64;
        let mut last_t = std::time::Instant::now();
        let mut speed_bps = 0u64;
        while !stop2.load(Ordering::Relaxed) {
            let received = dir_total_size(&dest2);
            let now = std::time::Instant::now();
            let dt = now.duration_since(last_t).as_secs_f64();
            if dt >= 0.5 {
                let delta = received.saturating_sub(last);
                speed_bps = if dt > 0.0 { (delta as f64 / dt) as u64 } else { 0 };
                last = received;
                last_t = now;
            }
            let _ = ch.send(PipelineEvent::DownloadProgress {
                step: step2.clone(),
                received,
                total,
                speed_bps,
            });
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    });

    let status = child.wait().map_err(|e| AppError::Io(e.to_string()));
    stop.store(true, Ordering::Relaxed);
    let _ = poller.join();
    let status = status?;
    if !status.success() {
        let mut raw = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut raw);
        }
        let err = clean_ps_error(&raw);
        return Err(AppError::Io(if err.is_empty() {
            format!("解压退出码 {:?}", status.code())
        } else {
            err
        }));
    }
    // 解压完成：补发一次 100% 进度（快速解压时轮询可能未捕获中间值，避免界面停留在 0%）。
    let _ = channel.send(PipelineEvent::DownloadProgress {
        step: step.to_string(),
        received: total,
        total,
        speed_bps: 0,
    });
    Ok(())
}

/// 解压 zip 到 dest，实时推送进度事件（step="extract"），先清空 dest 避免残留旧文件。
/// 优先使用 tar.exe（Win10 1803+ 自带 bsdtar，支持长路径，与便携打包一致），
/// 不可用或失败时回退 PowerShell `Expand-Archive`（两者均无黑框、带进度）。
pub fn extract_zip_with_progress(
    zip: &Path,
    dest: &Path,
    channel: &Channel<PipelineEvent>,
    step: &str,
) -> Result<(), AppError> {
    let (_count, total) = zip_uncompressed_totals(zip).unwrap_or((0, 0));
    let clear_dest = |dest: &Path| -> Result<(), AppError> {
        if dest.exists() {
            std::fs::remove_dir_all(dest).map_err(|e| AppError::Io(e.to_string()))?;
        }
        std::fs::create_dir_all(dest).map_err(|e| AppError::Io(e.to_string()))
    };
    clear_dest(dest)?;

    let zip_s = zip.to_string_lossy().to_string();
    let dest_s = dest.to_string_lossy().to_string();
    // 1) tar：加 CREATE_NO_WINDOW 避免弹出黑色控制台窗口。
    let mut tar_cmd = std::process::Command::new("tar.exe");
    tar_cmd.args(["-xf", &zip_s, "-C", &dest_s]);
    if run_extract_with_progress(&mut tar_cmd, dest, total, channel, step).is_ok() {
        return Ok(());
    }
    // 2) 回退 PowerShell Expand-Archive（同样带进度；先清空避免与 tar 半成品混用）。
    clear_dest(dest)?;
    let script = r#"
[Console]::OutputEncoding=[System.Text.Encoding]::UTF8
Expand-Archive -Path '@@ZIP@@' -DestinationPath '@@DEST@@' -Force
"#;
    let script = script
        .replace("@@ZIP@@", &ps_escape(&zip_s))
        .replace("@@DEST@@", &ps_escape(&dest_s));
    run_extract_with_progress(
        std::process::Command::new("powershell.exe").args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script.as_str(),
        ]),
        dest,
        total,
        channel,
        step,
    )
}

/// 在解压目录下定位含 `node_modules\.bin\dsh.cmd` 的根目录。
/// 兼容 zip 顶层带一个包装目录的情况（如 `deepseek-harness-pkg-windows/`）。
pub fn locate_dsh_root(dir: &Path) -> Result<PathBuf, AppError> {
    let direct = dir.join("node_modules").join(".bin").join("dsh.cmd");
    if direct.is_file() {
        return Ok(dir.to_path_buf());
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        let subdirs: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.path())
            .collect();
        if subdirs.len() == 1 {
            let nested = subdirs[0].join("node_modules").join(".bin").join("dsh.cmd");
            if nested.is_file() {
                return Ok(subdirs[0].clone());
            }
        }
    }
    Err(AppError::NotValidPrebuilt(dir.to_string_lossy().to_string()))
}

/// 校验解压后的预构建内核完整性：关键入口文件（dsh.cmd 与 CLI 主入口）必须存在，
/// 防止半解压 / 长路径截断等导致「安装成功但无法启动」。
pub fn verify_prebuilt_root(root: &Path) -> Result<(), AppError> {
    let dsh_cmd = root.join("node_modules").join(".bin").join("dsh.cmd");
    let bin_js = root
        .join("node_modules")
        .join("@deepseek-ai")
        .join("dsh")
        .join("lib")
        .join("bin.js");
    if !dsh_cmd.is_file() || !bin_js.is_file() {
        return Err(AppError::NotValidPrebuilt(root.to_string_lossy().to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_escape_doubles_single_quotes() {
        assert_eq!(ps_escape("a'b"), "a''b");
        assert_eq!(ps_escape("plain"), "plain");
    }

    #[test]
    fn clean_ps_error_strips_cmdlet_prefix_and_noise() {
        let raw = "Invoke-WebRequest : 无法连接到远程服务器\r\n+ CategoryInfo          : InvalidOperation: (:) [Invoke-WebRequest], WebException\r\n+ FullyQualifiedErrorId : WebCmdletWebResponseException,Microsoft.PowerShell.Commands.InvokeWebRequestCommand";
        let clean = clean_ps_error(raw);
        assert_eq!(clean, "无法连接到远程服务器");
        // 不含命令名前缀与噪音行。
        assert!(!clean.contains("Invoke-WebRequest"));
        assert!(!clean.contains("CategoryInfo"));
    }

    #[test]
    fn clean_ps_error_keeps_inner_colon() {
        // 消息本身包含冒号时只去掉行首的命令前缀。
        let raw = "Invoke-RestMethod : The remote server returned an error: (404) Not Found";
        assert_eq!(
            clean_ps_error(raw),
            "The remote server returned an error: (404) Not Found"
        );
    }

    #[test]
    fn clean_ps_error_empty_input() {
        assert_eq!(clean_ps_error(""), "");
        assert_eq!(clean_ps_error("  \r\n  \n"), "");
    }

    #[test]
    fn clean_ps_error_truncates_long_lines() {
        let long = "x".repeat(1000);
        let clean = clean_ps_error(&long);
        assert!(clean.chars().count() <= 301);
        assert!(clean.ends_with('…'));
    }

    /// 构造一个最小可解析的「存储(method 0)」zip（CRC 置 0；本测试仅解析元数据，不校验 CRC）。
    fn build_test_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut local = Vec::new();
        let mut central = Vec::new();
        let mut offset = 0u32;
        for (name, data) in entries {
            let nb = name.as_bytes();
            let len = data.len() as u32;
            local.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
            local.extend_from_slice(&20u16.to_le_bytes());
            local.extend_from_slice(&0u16.to_le_bytes());
            local.extend_from_slice(&0u16.to_le_bytes());
            local.extend_from_slice(&0u16.to_le_bytes());
            local.extend_from_slice(&0u16.to_le_bytes());
            local.extend_from_slice(&0u32.to_le_bytes());
            local.extend_from_slice(&len.to_le_bytes());
            local.extend_from_slice(&len.to_le_bytes());
            local.extend_from_slice(&(nb.len() as u16).to_le_bytes());
            local.extend_from_slice(&0u16.to_le_bytes());
            local.extend_from_slice(nb);
            local.extend_from_slice(data);

            central.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u32.to_le_bytes());
            central.extend_from_slice(&len.to_le_bytes());
            central.extend_from_slice(&len.to_le_bytes());
            central.extend_from_slice(&(nb.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u32.to_le_bytes());
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(nb);

            offset += 30 + nb.len() as u32 + len;
        }
        let cd_offset = local.len() as u32;
        let cd_size = central.len() as u32;
        let total = entries.len() as u16;
        let mut out = local;
        out.extend_from_slice(&central);
        out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&total.to_le_bytes());
        out.extend_from_slice(&total.to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    #[test]
    fn zip_uncompressed_totals_reads_central_directory() {
        let zip = std::env::temp_dir().join(format!("dsh-zip-{}.zip", std::process::id()));
        let bytes = build_test_zip(&[
            ("a.txt", b"hello"),
            ("dir/b.txt", b"world!"),
        ]);
        std::fs::write(&zip, bytes).unwrap();
        // 条目数 = 2；未压缩总字节 = 5 + 6 = 11。
        assert_eq!(zip_uncompressed_totals(&zip), Some((2, 11)));
        std::fs::remove_file(&zip).ok();
    }

    #[test]
    fn zip_uncompressed_totals_missing_file_is_none() {
        let zip = std::env::temp_dir().join(format!("dsh-zip-missing-{}.zip", std::process::id()));
        assert_eq!(zip_uncompressed_totals(&zip), None);
    }
}
