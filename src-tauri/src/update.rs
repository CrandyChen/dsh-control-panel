//! 更新流程（双模式）。
//!
//! - **source**：`git pull --ff-only` → `pnpm install` → `pnpm run build`。
//! - **prebuilt**：下载最新 `deepseek-harness-pkg-windows.zip` → 重新解压到 `dsh` 子目录。

use std::path::PathBuf;

use tauri::ipc::Channel;
use tauri::AppHandle;

use crate::config::{self, mode2_install_dir};
use crate::detect::{is_valid_prebuilt, is_valid_repo, read_pkg_version, read_version};
use crate::error::AppError;
use crate::logging::Logger;
use crate::process::{run_pipeline, PipelineEvent, Step};
use crate::version::read_commit;

/// 更新已安装的 DeepSeek Harness。要求：已安装；web 服务若在运行会先自动停止。
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

/// 源码更新：git pull → pnpm install → pnpm run build。
fn update_source(
    app: &AppHandle,
    path: &PathBuf,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    // git pull 前先检查仓库主机可达性（网络不可达直接报错，不执行 git 操作）。
    crate::net::ensure_repo_reachable().map_err(|e| e.friendly())?;

    // 更新前自动停止 web 服务（避免文件占用，更新完成后由用户重新启动）。
    if crate::web::port_in_use(crate::config::WEB_PORT) {
        let _ = crate::web::stop_web(app);
    }

    let steps = vec![
        Step {
            id: "pull",
            title: "拉取最新代码（git pull --ff-only）",
            program: "git",
            args: vec!["pull".into(), "--ff-only".into()],
            cwd: Some(path.clone()),
            envs: vec![("GIT_TERMINAL_PROMPT", "0".into())],
        },
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

    match run_pipeline(&steps, channel, logger) {
        Ok(()) => {}
        Err(e) => {
            let mut msg = e.friendly();
            if let AppError::StepFailed { step, .. } = &e {
                // 步骤标题已本地化：中英文均内嵌 `git pull` 命令串，据此判断拉取步骤。
                if step.contains("git pull") {
                    msg.push_str(&crate::i18n::t("update.pull.hint"));
                }
            }
            return Err(msg);
        }
    }

    let mut cfg = config::load_config(app);
    cfg.installed_version = read_version(path);
    cfg.installed_commit = read_commit(path);
    cfg.last_updated_at = Some(config::now_string());
    cfg.update_available = false;
    cfg.latest_commit = None;
    cfg.latest_subject = None;
    config::save_config(app, &cfg).map_err(|e| e)?;

    logger.info(&crate::i18n::t("log.update_done"));
    Ok(())
}

/// 预构建更新：下载最新 release zip → 重新解压到 dsh 子目录。
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
    let dest = mode2_install_dir();
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
    let version = read_pkg_version(&root).unwrap_or(release.tag.clone());

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
    cfg.install_dir = Some(root_str);
    cfg.install_mode = "prebuilt".to_string();
    cfg.installed_version = Some(version);
    cfg.installed_commit = None;
    cfg.last_updated_at = Some(config::now_string());
    cfg.update_available = false;
    cfg.latest_commit = None;
    cfg.latest_subject = None;
    config::save_config(app, &cfg).map_err(|e| e)?;

    let _ = channel.send(PipelineEvent::Finished { ok: true });
    logger.info(&crate::i18n::t("log.update_done"));
    Ok(())
}
