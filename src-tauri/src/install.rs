//! 安装流程（双模式）。
//!
//! - **source（从官方源码安装）**：网络预检 → git clone（到 `<exe_dir>\dsh-src-<版本>`）→
//!   `pnpm install`（保留 node_modules 增量）→ `pnpm run build`；源码更新在未保留旧版本时
//!   走 `git fetch/reset` 就地更新 + 增量构建（见 `update_source_in_place`）。
//! - **prebuilt（预构建内核，默认）**：GitHub 拉取最新
//!   `deepseek-harness-pkg-windows.zip`，解压到程序运行目录下的 `dsh-<版本>` 子目录。
//!   不需要外部 git / pnpm / node。

use tauri::ipc::Channel;
use tauri::AppHandle;

use crate::config;
use crate::detect::{is_valid_repo, read_pkg_version, read_version};
use crate::error::AppError;
use crate::logging::Logger;
use crate::process::{run_pipeline, PipelineEvent, Step};
use crate::version::read_commit;

/// 安装入口。source 使用程序运行目录下的仓库子目录（仅最新版）；prebuilt 安装指定
/// 版本（`version` 为归一化版本号，None 表示最新发布版），解压到该版本独立目录。
pub fn install(
    app: &AppHandle,
    mode: &str,
    version: Option<String>,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    match mode {
        "source" => install_from_source(app, channel, logger),
        _ => install_prebuilt(app, version, channel, logger),
    }
}

/// 源码安装：克隆官方仓库到版本化目录 → pnpm install → pnpm run build。
/// 运行环境（node/pnpm）在克隆仓库的同时并行下载。父目录固定为程序运行目录。
fn install_from_source(
    app: &AppHandle,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    install_source_latest(app, channel, logger)
}

/// 安装「最新版」源码内核到版本化目录 `<exe_dir>\dsh-src-<版本>`。
///
/// 版本号在克隆完成后读取（clone 到暂存目录），再据此重命名到最终目录；供
/// 初次源码安装与「源码更新」共用。完成后注册 `source-<version>` 内核并设为活动内核。
pub fn install_source_latest(
    app: &AppHandle,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    // git clone 前先检查仓库主机可达性（网络不可达直接报错，不执行 git 操作）。
    crate::net::ensure_repo_reachable().map_err(|e| e.friendly())?;

    let parent = config::mode1_default_parent();
    if !parent.exists() {
        std::fs::create_dir_all(&parent).map_err(|e| AppError::Io(e.to_string()).friendly())?;
    }

    // 并行下载运行环境（node/pnpm），与仓库克隆互不阻塞。
    let ch2 = channel.clone();
    let lg2 = logger.clone();
    let runtime_task = std::thread::spawn(move || crate::runtime::ensure_runtime(&ch2, &lg2));

    // 清理上次残留暂存目录，克隆到暂存目录后再按版本重命名。
    let staging = config::source_staging_dir();
    let _ = std::fs::remove_dir_all(&staging);

    let _ = channel.send(PipelineEvent::StepStarted {
        id: "clone".into(),
        title: crate::i18n::t("step.clone"),
    });
    let clone_result = crate::gitops::clone_with_progress(
        &crate::config::repo_url(),
        &staging,
        channel,
        "clone",
    )
    .map_err(|e| e.friendly());
    let _ = channel.send(PipelineEvent::StepFinished {
        id: "clone".into(),
        exit_code: clone_result.as_ref().map(|_| 0).unwrap_or(-1),
    });
    if let Err(msg) = clone_result {
        // 克隆失败：等待运行环境线程结束，避免残留进度事件。
        let _ = runtime_task.join();
        return Err(msg);
    }

    // 读取克隆得到的版本（apps/cli/package.json），据此确定最终目录名。
    let version =
        read_version(&staging).unwrap_or_else(|| "unknown".to_string());
    let target = config::source_install_dir(&version);
    let target_str = target.to_string_lossy().to_string();

    if is_valid_repo(&target) {
        let _ = std::fs::remove_dir_all(&staging);
        let _ = runtime_task.join();
        return Err(AppError::AlreadyInstalled(target_str).friendly());
    }
    // 目标目录存在但非法（非仓库残留）→ 清理后替换。
    if target.exists() {
        let _ = std::fs::remove_dir_all(&target);
    }
    std::fs::rename(&staging, &target)
        .map_err(|e| AppError::Io(format!("无法将克隆目录重命名为 {target_str}: {e}")).friendly())?;

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
    // 源码安装/更新的详细输出仅落盘（与预构建一致：主界面只显示步骤节点，不刷屏）。
    run_pipeline(&steps, channel, logger, false).map_err(|e| e.friendly())?;

    let mut cfg = config::load_config(app);
    cfg.install_dir = Some(target_str.clone());
    cfg.install_mode = "source".to_string();
    cfg.installed_version = read_version(&target);
    cfg.installed_commit = read_commit(&target);
    cfg.last_updated_at = Some(config::now_string());
    cfg.update_available = false;
    cfg.latest_commit = None;
    cfg.latest_subject = None;
    register_source_kernel(&mut cfg, &target, &target_str);
    config::save_config(app, &cfg).map_err(|e| e)?;

    logger.info(&crate::i18n::t_fmt("log.install_done", &[&target_str]));
    Ok(())
}

/// 登记源码模式内核（唯一一份，id 固定为 "source"）并设为活动内核。
fn register_source_kernel(cfg: &mut config::AppConfig, target: &std::path::Path, dir: &str) {
    let version = read_version(target).unwrap_or_else(|| "unknown".to_string());
    let id = config::kernel_id("source", &version);
    config::upsert_kernel(
        cfg,
        config::KernelInstall {
            id: id.clone(),
            mode: "source".to_string(),
            version,
            install_dir: dir.to_string(),
            commit: read_commit(target),
            installed_at: config::now_string(),
        },
    );
    config::set_active_kernel(cfg, &id);
}

/// 源码内核**就地更新**（升级后不保留当前版本的场景）：在当前源码目录 `git fetch → reset`
/// 到默认分支，保留 `node_modules` 做**增量** `pnpm install` + `pnpm run build`，完成后按
/// 新版本号把目录改名为 `<exe_dir>\dsh-src-<新版本>`，更新注册表。
///
/// 相比「全新 clone + 全量安装」显著缩短构建时间；目录整体改名（含 `.git`）后 git 仍可用。
pub fn update_source_in_place(
    app: &AppHandle,
    dir: &str,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    crate::net::ensure_repo_reachable().map_err(|e| e.friendly())?;
    crate::runtime::ensure_runtime(channel, logger)?;

    let dir_buf = std::path::PathBuf::from(dir);
    if !is_valid_repo(&dir_buf) {
        return Err(AppError::NotInstalled.friendly());
    }

    // git 操作：fetch → reset 到 origin 默认分支（丢弃本地改动，与全新安装行为一致）。
    let _ = channel.send(PipelineEvent::StepStarted {
        id: "pull".into(),
        title: crate::i18n::t("step.pull"),
    });
    let pull_result = (|| -> Result<(), String> {
        crate::gitops::fetch(&dir_buf).map_err(|e| e.friendly())?;
        let branch = crate::version::default_branch(&dir_buf).map_err(|e| e.friendly())?;
        crate::gitops::reset_hard(&dir_buf, &format!("origin/{branch}"))
            .map_err(|e| e.friendly())?;
        Ok(())
    })();
    let _ = channel.send(PipelineEvent::StepFinished {
        id: "pull".into(),
        exit_code: pull_result.as_ref().map(|_| 0).unwrap_or(-1),
    });
    if let Err(msg) = pull_result {
        return Err(format!("{msg}{}", crate::i18n::t("update.pull.hint")));
    }

    // 读取新版本，据此决定是否把目录改名。
    let new_version = read_version(&dir_buf).unwrap_or_else(|| "unknown".to_string());
    let target = config::source_install_dir(&new_version);
    let target_str = target.to_string_lossy().to_string();
    let work_dir = if target_str != dir {
        // 目标目录若已是有效仓库（如另一版本源码内核）→ 报错（不应发生，update 侧已判重）。
        if is_valid_repo(&target) {
            return Err(AppError::AlreadyInstalled(target_str).friendly());
        }
        if target.exists() {
            let _ = std::fs::remove_dir_all(&target);
        }
        std::fs::rename(&dir_buf, &target).map_err(|e| {
            AppError::Io(format!("无法将源码目录改名为 {target_str}: {e}")).friendly()
        })?;
        target
    } else {
        dir_buf
    };

    // 增量安装依赖 + 构建（保留 node_modules；详细输出仅落盘，与预构建一致）。
    let steps = vec![
        Step {
            id: "install",
            title: "安装依赖（pnpm install）",
            program: "pnpm.cmd",
            args: vec!["install".into()],
            cwd: Some(work_dir.clone()),
            envs: vec![],
        },
        Step {
            id: "build",
            title: "构建（pnpm run build）",
            program: "pnpm.cmd",
            args: vec!["run".into(), "build".into()],
            cwd: Some(work_dir.clone()),
            envs: vec![],
        },
    ];
    run_pipeline(&steps, channel, logger, false).map_err(|e| e.friendly())?;

    // 更新注册表：移除该路径（原目录）对应的旧源码记录，登记新版本并设为活动内核。
    let version = read_version(&work_dir).unwrap_or_else(|| new_version.clone());
    let id = config::kernel_id("source", &version);
    let work_str = work_dir.to_string_lossy().to_string();
    let commit = read_commit(&work_dir);
    let mut cfg = config::load_config(app);
    cfg.installed_kernels.retain(|k| !(k.mode == "source" && k.install_dir == dir));
    config::upsert_kernel(
        &mut cfg,
        config::KernelInstall {
            id: id.clone(),
            mode: "source".to_string(),
            version: version.clone(),
            install_dir: work_str.clone(),
            commit: commit.clone(),
            installed_at: config::now_string(),
        },
    );
    config::set_active_kernel(&mut cfg, &id);
    cfg.install_dir = Some(work_str.clone());
    cfg.install_mode = "source".to_string();
    cfg.installed_version = Some(version.clone());
    cfg.installed_commit = commit;
    cfg.last_updated_at = Some(config::now_string());
    cfg.update_available = false;
    cfg.latest_commit = None;
    cfg.latest_subject = None;
    config::save_config(app, &cfg).map_err(|e| e)?;

    logger.info(&crate::i18n::t("log.update_done"));
    Ok(())
}

/// 预构建内核安装：下载指定版本（或最新）release zip → 解压到该版本独立目录
/// `<exe_dir>\dsh-<version>`。失败先等运行环境线程结束，避免残留进度事件。
/// 完成后注册内核记录并校准 `~/.dsh/profiles/*` 的 bundle 列表。
fn install_prebuilt(
    app: &AppHandle,
    version: Option<String>,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    let _ = channel.send(PipelineEvent::StepStarted {
        id: "download".into(),
        title: crate::i18n::t("step.download"),
    });
    logger.file_only_info(&crate::i18n::t("log.prebuilt_start"));

    // 并行下载运行环境（node/pnpm），与内核下载互不阻塞。
    let ch2 = channel.clone();
    let lg2 = logger.clone();
    let runtime_task = std::thread::spawn(move || crate::runtime::ensure_runtime(&ch2, &lg2));

    // 内核步骤在闭包内执行：失败也先等运行环境线程结束，避免其继续向界面推送残留进度事件。
    let kernel_result = (|| -> Result<(), String> {
        // 解析目标版本与下载地址：指定版本 → 该 release；否则最新发布版。
        let (_tag, url, size, dir_version) = match version.as_deref() {
            Some(v) => {
                let rel = crate::prebuilt::release_by_version(v).map_err(|e| e.friendly())?;
                (rel.tag, rel.url, rel.size, v.to_string())
            }
            None => {
                let rel = crate::prebuilt::latest_release().map_err(|e| e.friendly())?;
                let norm = crate::version::normalized_tag_version(&rel.tag);
                (rel.tag, rel.url, rel.size, norm)
            }
        };
        let dest = config::kernel_install_dir("prebuilt", &dir_version);

        crate::prebuilt::download_asset(&url, &std::env::temp_dir().join("dsh-prebuilt.zip"), size, channel, "download")
            .map_err(|e| e.friendly())?;

        let _ = channel.send(PipelineEvent::StepFinished {
            id: "download".into(),
            exit_code: 0,
        });

        let _ = channel.send(PipelineEvent::StepStarted {
            id: "extract".into(),
            title: crate::i18n::t("step.extract"),
        });
        crate::prebuilt::extract_zip_with_progress(
            &std::env::temp_dir().join("dsh-prebuilt.zip"),
            &dest,
            channel,
            "extract",
        )
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
        let installed_version = read_pkg_version(&root).unwrap_or_else(|| dir_version.clone());

        // profile bundle 校准：移除其它发行版残留的、当前内核无法解析的 bundle。
        log_reconcile(&root, logger);

        let mut cfg = config::load_config(app);
        cfg.install_dir = Some(root_str.clone());
        cfg.install_mode = "prebuilt".to_string();
        cfg.installed_version = Some(installed_version.clone());
        cfg.installed_commit = None;
        cfg.last_updated_at = Some(config::now_string());
        cfg.update_available = false;
        cfg.latest_commit = None;
        cfg.latest_subject = None;
        // 登记内核（id 按归一化版本），并设为活动内核。
        let kernel_id = config::kernel_id("prebuilt", &installed_version);
        config::upsert_kernel(
            &mut cfg,
            config::KernelInstall {
                id: kernel_id.clone(),
                mode: "prebuilt".to_string(),
                version: installed_version.clone(),
                install_dir: root_str.clone(),
                commit: None,
                installed_at: config::now_string(),
            },
        );
        config::set_active_kernel(&mut cfg, &kernel_id);
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
    fn source_install_target_is_versioned_dir() {
        // 新版源码安装目标 = 程序运行目录 + dsh-src-<版本>（版本化，可多版本并存）。
        let target = config::source_install_dir("0.1.2-alpha.2");
        assert!(target.is_absolute());
        assert!(target.to_string_lossy().ends_with("dsh-src-0.1.2-alpha.2"));
        // 暂存目录固定为 dsh-src-work。
        assert!(config::source_staging_dir().to_string_lossy().ends_with("dsh-src-work"));
        // 旧版单目录仍可识别（向后兼容）。
        assert!(config::source_install_dir_legacy()
            .to_string_lossy()
            .ends_with(&config::repo_dir_name().as_str()));
    }

    #[test]
    fn source_default_parent_is_exe_dir() {
        let default = config::mode1_default_parent();
        // 程序运行目录（exe 所在目录）应存在（当前进程 exe）。
        assert!(default.is_absolute());
        assert!(!default.as_os_str().is_empty());
    }
}
