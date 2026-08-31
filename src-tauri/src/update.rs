//! 更新流程（双模式）。
//!
//! - **source**：`git pull --ff-only` → `pnpm install` → `pnpm run build`。
//! - **prebuilt**：下载最新 `deepseek-harness-pkg-windows.zip` → 重新解压到 `dsh` 子目录。

use std::path::PathBuf;

use tauri::ipc::Channel;
use tauri::AppHandle;

use crate::config::{self};
use crate::detect::{is_valid_prebuilt, is_valid_repo, read_pkg_version, read_version};
use crate::error::AppError;
use crate::logging::Logger;
use crate::process::{run_pipeline, PipelineEvent, Step};
use crate::version::read_commit;

/// 更新已安装的 DeepSeek Harness。要求：已安装；web 服务若在运行会先自动停止。
/// 预构建模式把最新发布版作为新版本内核安装并设为活动内核（旧版本保留，可切换/卸载）；
/// 源码模式原地更新并刷新对应内核记录。
pub fn update(
    app: &AppHandle,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    let cfg = config::load_config(app);
    let mode = cfg.install_mode.clone();
    let dir = cfg
        .install_dir
        .clone()
        .ok_or_else(|| AppError::NotInstalled.friendly())?;
    let path = PathBuf::from(&dir);
    if mode == "source" {
        if !is_valid_repo(&path) {
            return Err(AppError::NotInstalled.friendly());
        }
        update_source(app, &path, channel, logger)
    } else {
        if !is_valid_prebuilt(&path) {
            return Err(AppError::NotInstalled.friendly());
        }
        update_prebuilt(app, channel, logger)
    }
}

/// 源码更新：fetch → reset 到默认分支 → pnpm install → pnpm run build（git 由 gitops 完成）。
fn update_source(
    app: &AppHandle,
    path: &PathBuf,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    // git 操作前先检查仓库主机可达性（网络不可达直接报错，不执行 git 操作）。
    crate::net::ensure_repo_reachable().map_err(|e| e.friendly())?;

    // 更新前自动停止 web 服务（避免文件占用，更新完成后由用户重新启动）。
    if crate::web::port_in_use(crate::config::WEB_PORT) {
        let _ = crate::web::stop_web(app);
    }

    // 更新前确保运行环境（node/pnpm）就绪，供后续 install/build 使用。
    crate::runtime::ensure_runtime(channel, logger)?;

    // git pull --ff-only 等价：fetch 后硬重置到 origin 默认分支。
    let _ = channel.send(PipelineEvent::StepStarted {
        id: "pull".into(),
        title: crate::i18n::t("step.pull"),
    });
    let pull_result = (|| -> Result<(), String> {
        crate::gitops::fetch(path).map_err(|e| e.friendly())?;
        let branch = crate::version::default_branch(path).map_err(|e| e.friendly())?;
        crate::gitops::reset_hard(path, &format!("origin/{branch}")).map_err(|e| e.friendly())?;
        Ok(())
    })();
    let _ = channel.send(PipelineEvent::StepFinished {
        id: "pull".into(),
        exit_code: pull_result.as_ref().map(|_| 0).unwrap_or(-1),
    });
    if let Err(msg) = pull_result {
        return Err(format!("{msg}{}", crate::i18n::t("update.pull.hint")));
    }

    let steps = vec![
        Step {
            id: "install",
            title: "安装依赖（pnpm install）",
            program: "pnpm.cmd",
            args: vec!["install".into()],
            cwd: Some(path.clone()),
            envs: vec![],
        },
        Step {
            id: "build",
            title: "构建（pnpm run build）",
            program: "pnpm.cmd",
            args: vec!["run".into(), "build".into()],
            cwd: Some(path.clone()),
            envs: vec![],
        },
    ];

    run_pipeline(&steps, channel, logger).map_err(|e| e.friendly())?;

    let mut cfg = config::load_config(app);
    cfg.installed_version = read_version(path);
    cfg.installed_commit = read_commit(path);
    cfg.last_updated_at = Some(config::now_string());
    cfg.update_available = false;
    cfg.latest_commit = None;
    cfg.latest_subject = None;
    // 刷新源码内核记录（同 id 覆盖），并保持为活动内核。
    let version = cfg.installed_version.clone().unwrap_or_else(|| "unknown".to_string());
    let id = config::kernel_id("source", &version);
    let install_dir = cfg.install_dir.clone().unwrap_or_default();
    let commit = cfg.installed_commit.clone();
    config::upsert_kernel(
        &mut cfg,
        config::KernelInstall {
            id: id.clone(),
            mode: "source".to_string(),
            version,
            install_dir,
            commit,
            installed_at: config::now_string(),
        },
    );
    config::set_active_kernel(&mut cfg, &id);
    config::save_config(app, &cfg).map_err(|e| e)?;

    logger.info(&crate::i18n::t("log.update_done"));
    Ok(())
}

/// 预构建更新：下载最新 release zip → 解压到该版本独立目录，并作为新内核激活。
fn update_prebuilt(
    app: &AppHandle,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    // 更新前自动停止 web 服务（避免文件占用，更新完成后由用户重新启动）。
    if crate::web::port_in_use(crate::config::WEB_PORT) {
        let _ = crate::web::stop_web(app);
    }

    let _ = channel.send(PipelineEvent::StepStarted {
        id: "download".into(),
        title: crate::i18n::t("step.download"),
    });
    logger.info(&crate::i18n::t("log.prebuilt_update_start"));

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
    let version = read_pkg_version(&root).unwrap_or_else(|| norm.clone());

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
    cfg.last_updated_at = Some(config::now_string());
    cfg.update_available = false;
    cfg.latest_commit = None;
    cfg.latest_subject = None;
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

    let _ = channel.send(PipelineEvent::Finished { ok: true });
    logger.info(&crate::i18n::t("log.update_done"));
    Ok(())
}
