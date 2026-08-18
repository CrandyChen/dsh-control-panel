//! 更新流程：git pull --ff-only → pnpm install → pnpm run build。

use std::path::PathBuf;

use tauri::ipc::Channel;
use tauri::AppHandle;

use crate::config;
use crate::detect::{is_valid_repo, read_version};
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
    // git pull 前先检查仓库主机可达性（网络不可达直接报错，不执行 git 操作）。
    crate::net::ensure_repo_reachable().map_err(|e| e.friendly())?;

    let cfg = config::load_config(app);
    let dir = cfg
        .install_dir
        .clone()
        .ok_or_else(|| AppError::NotInstalled.friendly())?;
    let path = PathBuf::from(&dir);
    if !is_valid_repo(&path) {
        return Err(AppError::NotInstalled.friendly());
    }
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
                if step.contains("拉取") {
                    msg.push_str(&crate::i18n::t("update.pull.hint"));
                }
            }
            return Err(msg);
        }
    }

    let mut cfg = config::load_config(app);
    cfg.installed_version = read_version(&path);
    cfg.installed_commit = read_commit(&path);
    cfg.last_updated_at = Some(config::now_string());
    cfg.update_available = false;
    cfg.latest_commit = None;
    cfg.latest_subject = None;
    config::save_config(app, &cfg).map_err(|e| e)?;

    logger.info("✅ 更新完成");
    Ok(())
}
