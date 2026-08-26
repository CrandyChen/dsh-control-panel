//! 安装流程（双模式）。
//!
//! - **source（从官方源码安装）**：网络预检 → git clone（自动创建子目录）→ pnpm install →
//!   pnpm run build。用户选择**父目录**，默认程序运行目录；clone 目标 =
//!   `<父目录>\<repo 目录名>`（默认 `deepseek-harness`）。依赖外部 git + 内置 node/pnpm。
//! - **prebuilt（预构建内核，默认）**：GitHub 拉取最新
//!   `deepseek-harness-pkg-windows.zip`，解压到程序运行目录下的 `dsh` 子目录。
//!   不需要外部 git / pnpm / node。

use std::path::PathBuf;

use tauri::ipc::Channel;
use tauri::AppHandle;

use crate::config::{self, repo_dir_name};
use crate::detect::{is_valid_repo, read_pkg_version, read_version};
use crate::error::AppError;
use crate::logging::Logger;
use crate::process::{run_pipeline, PipelineEvent, Step};
use crate::version::read_commit;

/// 安装入口。`dir` 为父目录（source）或空（prebuilt 使用程序目录下的 dsh 子目录）。
pub fn install(
    app: &AppHandle,
    dir: &str,
    mode: &str,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    match mode {
        "source" => install_from_source(app, dir, channel, logger),
        _ => install_prebuilt(app, channel, logger),
    }
}

/// 源码安装：克隆官方仓库（gitops/libgit2）→ pnpm install → pnpm run build。
/// 运行环境（node/pnpm）在克隆仓库的同时并行下载。
fn install_from_source(
    app: &AppHandle,
    dir: &str,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    // git clone 前先检查仓库主机可达性（网络不可达直接报错，不执行 git 操作）。
    crate::net::ensure_repo_reachable().map_err(|e| e.friendly())?;

    let parent = if dir.trim().is_empty() {
        config::mode1_default_parent()
    } else {
        PathBuf::from(dir)
    };
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

    // 并行下载运行环境（node/pnpm），与仓库克隆互不阻塞。
    let ch2 = channel.clone();
    let lg2 = logger.clone();
    let runtime_task = std::thread::spawn(move || crate::runtime::ensure_runtime(&ch2, &lg2));

    // clone 步骤（gitops/libgit2，带进度）。
    let _ = channel.send(PipelineEvent::StepStarted {
        id: "clone".into(),
        title: crate::i18n::t("step.clone"),
    });
    let clone_result = crate::gitops::clone_with_progress(
        &crate::config::repo_url(),
        &target,
        channel,
        "clone",
    )
    .map_err(|e| e.friendly());
    let _ = channel.send(PipelineEvent::StepFinished {
        id: "clone".into(),
        exit_code: clone_result.as_ref().map(|_| 0).unwrap_or(-1),
    });
    clone_result?;

    // 等待运行环境下载完成（与克隆并行）。
    match runtime_task.join() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err("运行环境下载线程异常".to_string()),
    }

    let steps = vec![
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
    cfg.install_mode = "source".to_string();
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

/// 预构建内核安装：下载最新 release zip → 解压到程序目录下的 dsh 子目录。
/// 完成后校准 `~/.dsh/profiles/*` 的 bundle 列表（移除内核无法解析的条目）。
fn install_prebuilt(
    app: &AppHandle,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    let _ = channel.send(PipelineEvent::StepStarted {
        id: "download".into(),
        title: crate::i18n::t("step.download"),
    });
    logger.info(&crate::i18n::t("log.prebuilt_start"));

    // 并行下载运行环境（node/pnpm），与内核下载互不阻塞。
    let ch2 = channel.clone();
    let lg2 = logger.clone();
    let runtime_task = std::thread::spawn(move || crate::runtime::ensure_runtime(&ch2, &lg2));

    // 内核步骤在闭包内执行：失败也先等运行环境线程结束，避免其继续向界面推送残留进度事件。
    let kernel_result = (|| -> Result<(), String> {
        let release = crate::prebuilt::latest_release().map_err(|e| e.friendly())?;
        let tmp = std::env::temp_dir().join("dsh-prebuilt.zip");
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
        let dest = config::mode2_install_dir();
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
        log_reconcile(&root, logger);

        let mut cfg = config::load_config(app);
        cfg.install_dir = Some(root_str.clone());
        cfg.install_mode = "prebuilt".to_string();
        cfg.installed_version = Some(version);
        cfg.installed_commit = None;
        cfg.last_updated_at = Some(config::now_string());
        cfg.update_available = false;
        cfg.latest_commit = None;
        cfg.latest_subject = None;
        config::save_config(app, &cfg).map_err(|e| e)?;

        let _ = channel.send(PipelineEvent::Finished { ok: true });
        logger.info(&crate::i18n::t_fmt("log.prebuilt_done", &[&root_str]));
        Ok(())
    })();

    // 无论内核成败，都等待运行环境线程结束（避免残留事件干扰界面）。
    let runtime_err = match runtime_task.join() {
        Ok(Ok(())) => None,
        Ok(Err(e)) => Some(e),
        Err(_) => Some("运行环境下载线程异常".to_string()),
    };
    if let Some(e) = runtime_err {
        return Err(e);
    }
    kernel_result
}

/// 执行 profile bundle 校准并把移除结果写入日志（供安装/更新/修复共用）。
fn log_reconcile(install_dir: &std::path::Path, logger: &Logger) {
    for (profile, removed) in crate::plugin::reconcile_all_profiles(install_dir) {
        if !removed.is_empty() {
            logger.warn(&crate::i18n::t_fmt(
                "log.profile_reconcile",
                &[&profile, &removed.join("、")],
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_install_target_is_parent_plus_repo_dir() {
        // 源码安装目标 = 选择的父目录 + 仓库目录名（默认 deepseek-harness）。
        let parent = PathBuf::from(r"D:\dev");
        let target = parent.join(repo_dir_name());
        assert_eq!(target.to_string_lossy(), r"D:\dev\deepseek-harness");
    }

    #[test]
    fn source_default_parent_is_exe_dir() {
        let default = config::mode1_default_parent();
        // 程序运行目录（exe 所在目录）应存在（当前进程 exe）。
        assert!(default.is_absolute());
        assert!(!default.as_os_str().is_empty());
    }
}
