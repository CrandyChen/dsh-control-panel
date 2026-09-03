//! 更新流程（多内核）。
//!
//! 更新对话框列出所有已安装内核（安装方式 / 当前版本 / 新版本），用户勾选要更新的内核
//! （可跨安装方式多选），并可选「升级后保留当前版本」：
//!
//! - **不勾选保留**（默认）：安装该方式最新内核后，删除被勾选的旧版本内核；
//! - **勾选保留**：只新装最新内核，保留所有被勾选的当前版本；
//! - 同一种安装方式下勾选多个版本时，**只安装**该方式的一个最新内核。
//!
//! 支持预构建与源码同时更新：
//! - **prebuilt**：下载最新 `deepseek-harness-pkg-windows.zip` → 解压到 `dsh-<version>`；
//! - **source**：克隆官方仓库到 `dsh-src-<version>` → `pnpm install` → `pnpm run build`。

use std::path::PathBuf;

use tauri::ipc::Channel;
use tauri::AppHandle;

use crate::config::{self};
use crate::error::AppError;
use crate::logging::Logger;
use crate::process::PipelineEvent;

/// 更新已安装的 DeepSeek Harness 内核。
///
/// `selected` 为要更新的内核 id 列表（来自注册表，可跨安装方式多选）；
/// `keep_current` 为「升级后是否同时保留当前版本」（默认 false：删除被勾选的旧版本）。
pub fn update(
    app: &AppHandle,
    selected: Vec<String>,
    keep_current: bool,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    // 更新前自动停止 web 服务（避免文件占用，更新完成后由用户重新启动）。
    if crate::web::port_in_use(crate::config::WEB_PORT) {
        let _ = crate::web::stop_web(app);
    }

    let cfg = config::load_config(app);

    // 解析选中内核 id 为内核记录；找不到则报错。
    let mut selected_kernels: Vec<config::KernelInstall> = Vec::new();
    for id in &selected {
        let k = config::find_kernel(&cfg, id)
            .ok_or_else(|| AppError::NotInstalled.friendly())?;
        selected_kernels.push(k);
    }
    if selected_kernels.is_empty() {
        return Err(crate::i18n::t("update.no_selection"));
    }

    // 按安装方式分组（保序、去重）：每种方式只安装一次最新内核。
    let mut modes: Vec<String> = Vec::new();
    for k in &selected_kernels {
        if !modes.contains(&k.mode) {
            modes.push(k.mode.clone());
        }
    }

    // 为每种被选中的方式确定「最新版本」；若该方式的最新内核尚未安装则安装之。
    let mut target_versions: Vec<(String, String)> = Vec::new();
    for mode in &modes {
        let new_ver = latest_version_for_mode(&cfg, mode)?;
        target_versions.push((mode.clone(), new_ver.clone()));
        let already_installed = cfg
            .installed_kernels
            .iter()
            .any(|k| k.mode == *mode && k.version == new_ver);
        if !already_installed {
            if mode == "prebuilt" {
                install_prebuilt_latest(app, channel, logger)?;
            } else if !keep_current {
                // 源码更新（升级后不保留）：就地 pull + 增量构建 + 改名，避免全新 clone + 全量安装。
                // 取被选源码内核中版本最高者的目录就地更新。
                let src_dir = selected_kernels
                    .iter()
                    .filter(|k| k.mode == "source")
                    .max_by(|a, b| cmp_versions(&a.version, &b.version))
                    .map(|k| k.install_dir.clone());
                match src_dir {
                    Some(d) => crate::install::update_source_in_place(app, &d, channel, logger)?,
                    None => crate::install::install_source_latest(app, channel, logger)?,
                }
            } else {
                // 保留当前版本：需独立新目录（新克隆到 dsh-src-<新版本>）。
                crate::install::install_source_latest(app, channel, logger)?;
            }
        }
    }

    // 默认（不保留）：删除被勾选的、版本与该方式最新版本不同的旧内核（目录 + 注册表）。
    if !keep_current {
        // 先提示一次，让界面在删除大目录期间有反馈（避免长时间无进展的卡顿观感）。
        let _ = channel.send(PipelineEvent::Output {
            step: "cleanup".into(),
            stream: "stdout".into(),
            line: crate::i18n::t("update.cleanup_start"),
        });
        let mut cfg2 = config::load_config(app);
        for k in &selected_kernels {
            let new_ver = target_versions
                .iter()
                .find(|(m, _)| m == &k.mode)
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            if k.version != new_ver {
                let name = crate::uninstall::kernel_display_name(&k.mode, &k.version);
                let _ = channel.send(PipelineEvent::StepStarted {
                    id: "cleanup".into(),
                    title: crate::i18n::t_fmt("update.cleanup_step", &[&name]),
                });
                let dir = PathBuf::from(&k.install_dir);
                if dir.is_dir() {
                    logger.file_only_info(&crate::i18n::t_fmt(
                        "log.update_cleanup",
                        &[&name, &k.install_dir],
                    ));
                    let _ = std::fs::remove_dir_all(&dir);
                }
                config::remove_kernel(&mut cfg2, &k.id);
                let _ = channel.send(PipelineEvent::StepFinished {
                    id: "cleanup".into(),
                    exit_code: 0,
                });
            }
        }
        config::save_config(app, &cfg2).map_err(|e| e)?;
    }

    // 设定活动内核为已装内核中版本最高者，并清除更新标记。
    let mut cfg3 = config::load_config(app);
    if let Some(active) = config::latest_kernel(&cfg3) {
        config::set_active_kernel(&mut cfg3, &active.id);
    }
    cfg3.last_updated_at = Some(config::now_string());
    cfg3.update_available = false;
    cfg3.latest_commit = None;
    cfg3.latest_subject = None;
    config::save_config(app, &cfg3).map_err(|e| e)?;

    let _ = channel.send(PipelineEvent::Finished { ok: true });
    logger.info(&crate::i18n::t("log.update_done"));
    Ok(())
}

/// 语义化版本比较（优先 semver，解析失败退化为字典序）。用于挑选就地更新的源码目录。
fn cmp_versions(a: &str, b: &str) -> std::cmp::Ordering {
    match (semver::Version::parse(a), semver::Version::parse(b)) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        _ => a.cmp(b),
    }
}

/// 计算某安装方式的最新版本：预构建读最新 release 归一化版本；源码读已装源码目录的
/// 远程默认分支 CLI 版本（取注册表中第一个源码内核的目录）。
fn latest_version_for_mode(
    cfg: &config::AppConfig,
    mode: &str,
) -> Result<String, String> {
    if mode == "prebuilt" {
        let release = crate::prebuilt::latest_release().map_err(|e| e.friendly())?;
        Ok(crate::version::normalized_tag_version(&release.tag))
    } else {
        let dir = cfg
            .installed_kernels
            .iter()
            .find(|k| k.mode == "source")
            .map(|k| k.install_dir.clone())
            .ok_or_else(|| AppError::NotInstalled.friendly())?;
        crate::version::latest_source_version(std::path::Path::new(&dir))
            .map_err(|e| e.friendly())
    }
}

/// 预构建更新：下载最新 release zip → 解压到该版本独立目录，注册并设为活动内核。
/// 不发送 `Finished`（由 `update` 统一发送），也不清除更新标记（由 `update` 收尾）。
fn install_prebuilt_latest(
    app: &AppHandle,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    let _ = channel.send(PipelineEvent::StepStarted {
        id: "download".into(),
        title: crate::i18n::t("step.download"),
    });
    logger.file_only_info(&crate::i18n::t("log.prebuilt_update_start"));

    let release = crate::prebuilt::latest_release().map_err(|e| e.friendly())?;
    let norm = crate::version::normalized_tag_version(&release.tag);
    let dest = config::prebuilt_version_dir(&norm);
    let tmp = std::env::temp_dir().join("dsh-prebuilt-update.zip");
    crate::prebuilt::download_asset(&release.url, &tmp, release.size, channel, "download")
        .map_err(|e| e.friendly())?;
    let _ = channel.send(PipelineEvent::StepFinished {
        id: "download".into(),
        exit_code: 0,
    });

    let _ = channel.send(PipelineEvent::StepStarted {
        id: "extract".into(),
        title: crate::i18n::t("step.extract"),
    });
    crate::prebuilt::extract_zip_with_progress(&tmp, &dest, channel, "extract")
        .map_err(|e| e.friendly())?;
    let _ = channel.send(PipelineEvent::StepFinished {
        id: "extract".into(),
        exit_code: 0,
    });

    let root = crate::prebuilt::locate_dsh_root(&dest).map_err(|e| e.friendly())?;
    // 解压完整性校验：关键入口缺失说明解压不完整，删除损坏目录并报错。
    if let Err(e) = crate::prebuilt::verify_prebuilt_root(&root) {
        let _ = std::fs::remove_dir_all(&dest);
        return Err(e.friendly());
    }
    let root_str = root.to_string_lossy().to_string();
    let version = crate::detect::read_pkg_version(&root)
        .unwrap_or_else(|| norm.clone());

    // profile bundle 校准：移除其它发行版残留的、当前内核无法解析的 bundle。
    for (profile, removed) in crate::plugin::reconcile_all_profiles(&root) {
        if !removed.is_empty() {
            logger.warn(&crate::i18n::t_fmt(
                "log.profile_reconcile",
                &[&profile, &removed.join("、")],
            ));
        }
    }

    let mut cfg = config::load_config(app);
    cfg.install_dir = Some(root_str.clone());
    cfg.install_mode = "prebuilt".to_string();
    cfg.installed_version = Some(version.clone());
    cfg.installed_commit = None;
    // 以最新发布版作为新内核登记并激活（旧版本保留在注册表，可切换/卸载）。
    let kernel_id = config::kernel_id("prebuilt", &version);
    config::upsert_kernel(
        &mut cfg,
        config::KernelInstall {
            id: kernel_id.clone(),
            mode: "prebuilt".to_string(),
            version: version.clone(),
            install_dir: root_str.clone(),
            commit: None,
            installed_at: config::now_string(),
        },
    );
    config::set_active_kernel(&mut cfg, &kernel_id);
    config::save_config(app, &cfg).map_err(|e| e)?;

    logger.info(&crate::i18n::t("log.update_done"));
    Ok(())
}
