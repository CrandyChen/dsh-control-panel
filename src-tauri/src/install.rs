//! 安装流程：网络预检 → git clone（自动创建子目录）→ pnpm install → pnpm run build。
//!
//! 用户选择的是**父目录**，控制面板自动在父目录下创建 `<父目录>\<repo 目录名>`
//! （默认 `deepseek-harness`）并克隆；父目录是否非空不再校验（git clone 会创建新子目录）。

use std::path::PathBuf;

use tauri::ipc::Channel;
use tauri::AppHandle;

use crate::config::{self, repo_dir_name, repo_url};
use crate::detect::{is_valid_repo, read_version};
use crate::error::AppError;
use crate::logging::Logger;
use crate::process::{run_pipeline, PipelineEvent, Step};
use crate::version::read_commit;

/// 安装到指定父目录。要求：目标子目录（`<父目录>/<repo 目录名>`）不存在或为空，
/// 若已是有效仓库则拒绝并提示使用更新。
pub fn install(
    app: &AppHandle,
    dir: &str,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    // git clone 前先检查仓库主机可达性（网络不可达直接报错，不执行 git 操作）。
    crate::net::ensure_repo_reachable().map_err(|e| e.friendly())?;

    let parent = PathBuf::from(dir);
    let target = parent.join(repo_dir_name());
    let target_str = target.to_string_lossy().to_string();

    if target.exists() && !target.is_dir() {
        return Err(AppError::InvalidPath(target_str.clone()).friendly());
    }
    if is_valid_repo(&target) {
        return Err(AppError::AlreadyInstalled(target_str).friendly());
    }
    // 父目录不存在时先创建（git clone 会创建目标子目录，但父目录需先存在）。
    if !parent.exists() {
        std::fs::create_dir_all(&parent).map_err(|e| AppError::Io(e.to_string()).friendly())?;
    }

    let steps = vec![
        Step {
            id: "clone",
            title: "克隆仓库（git clone）",
            program: "git",
            args: vec!["clone".into(), repo_url(), target_str.clone()],
            cwd: Some(parent),
            envs: vec![("GIT_TERMINAL_PROMPT", "0".into())],
        },
        Step {
            id: "install",
            title: "安装依赖（pnpm install）",
            program: "pnpm.cmd",
            args: vec!["install".into()],
            cwd: Some(target.clone()),
            envs: vec![],
        },
        Step {
            id: "build",
            title: "构建（pnpm run build）",
            program: "pnpm.cmd",
            args: vec!["run".into(), "build".into()],
            cwd: Some(target.clone()),
            envs: vec![],
        },
    ];
    run_pipeline(&steps, channel, logger).map_err(|e| e.friendly())?;

    let mut cfg = config::load_config(app);
    cfg.install_dir = Some(target_str.clone());
    cfg.installed_version = read_version(&target);
    cfg.installed_commit = read_commit(&target);
    cfg.last_updated_at = Some(config::now_string());
    cfg.update_available = false;
    cfg.latest_commit = None;
    cfg.latest_subject = None;
    config::save_config(app, &cfg).map_err(|e| e)?;

    logger.info(&crate::i18n::t_fmt("log.install_done", &[&target_str]));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_target_is_parent_plus_repo_dir() {
        // 安装目标 = 用户选择的父目录 + 仓库目录名（默认 deepseek-harness）。
        let parent = PathBuf::from(r"D:\dev");
        let target = parent.join(repo_dir_name());
        assert_eq!(target.to_string_lossy(), r"D:\dev\deepseek-harness");
    }
}
