//! 修复安装：清理异常过程文件与状态，分级修复 DeepSeek Harness 安装并重新编译部署。
//!
//! 修复等级（按需升级，前一级失败才进入下一级）：
//! - **L1 清理**：停止 web 服务、清理残留进程、清理 git 锁与中断状态文件；
//! - **L2 重置重建**：`git fetch` → `git reset --hard origin/<默认分支>` → `git clean`
//!   → `pnpm install`（构建脚本被拦截时自动放行重试）→ `pnpm run build`；
//! - **L3 深度重建**：删除 `node_modules` 后重新 `pnpm install` + `pnpm run build`；
//! - **L4 profile 修复**：对 `~/.dsh/profiles/*` 中已初始化的 profile 执行
//!   `dsh plugin --profile <p> install`（best-effort，失败仅告警不阻断）；
//! - **L5 保底重装**：删除安装目录并重新 `git clone` + `pnpm install` + `pnpm run build`
//!   （相当于自动化的「卸载 + 重装」，仅在前几级全部失败时触发）。
//!
//! 说明：`~/.dsh` 中的配置、凭据与插件清单在修复过程中保留；安装目录内的未提交本地改动
//! 会被丢弃（前端确认对话框已向用户明示风险）。

use std::path::{Path, PathBuf};

use tauri::ipc::Channel;
use tauri::AppHandle;

use crate::config;
use crate::detect::is_valid_repo;
use crate::error::AppError;
use crate::logging::Logger;
use crate::plugin::{self, RunOutcome};
use crate::process::PipelineEvent;

// ─────────────────────────────── L1 清理 ───────────────────────────────

/// 收集 git 锁文件与中断状态文件（重建前清理，避免残留状态导致 git 命令挂起/失败）。
pub fn git_lock_files(dir: &Path) -> Vec<PathBuf> {
    let git = dir.join(".git");
    let mut out = Vec::new();
    if !git.is_dir() {
        return out;
    }
    let mut push = |p: PathBuf| {
        if p.exists() {
            out.push(p);
        }
    };
    push(git.join("index.lock"));
    push(git.join("shallow.lock"));
    push(git.join("packed-refs.lock"));
    push(git.join("MERGE_HEAD"));
    push(git.join("CHERRY_PICK_HEAD"));
    push(git.join("REBASE_HEAD"));
    push(git.join("REVERT_HEAD"));
    push(git.join("BISECT_LOG"));
    push(git.join("sequencer"));
    push(git.join("rebase-merge"));
    push(git.join("rebase-apply"));
    // refs 与 hooks 下递归收集 *.lock。
    for sub in ["refs", "hooks"] {
        let base = git.join(sub);
        if base.is_dir() {
            collect_lock_files(&base, &mut out);
        }
    }
    out
}

fn collect_lock_files(base: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(base) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect_lock_files(&p, out);
            } else if p.extension().map(|x| x == "lock").unwrap_or(false) && p.exists() {
                out.push(p);
            }
        }
    }
}

/// 清理残留进程：结束命令行中包含安装目录的 node / pnpm / cmd 进程（best-effort）。
#[cfg(windows)]
fn kill_stray_processes(dir: &str, logger: &Logger) {
    use std::process::Command;
    let script = r#"
$dir = $env:DSH_REPAIR_DIR
Get-CimInstance Win32_Process | Where-Object {
  $_.CommandLine -and $_.CommandLine.Contains($dir) -and ($_.Name -eq 'node.exe' -or $_.Name -like 'pnpm*' -or $_.Name -eq 'cmd.exe')
} | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
"#;
    let result = crate::process::no_window(
        Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("DSH_REPAIR_DIR", dir),
    )
    .output();
    match result {
        Ok(_) => logger.info(&crate::i18n::t("log.repair_kill_ok")),
        Err(e) => logger.warn(&crate::i18n::t_fmt("log.repair_kill_fail", &[&e.to_string()])),
    }
}

#[cfg(not(windows))]
fn kill_stray_processes(_dir: &str, _logger: &Logger) {}

// ─────────────────────────────── 步骤执行 ───────────────────────────────

/// 执行单步命令，输出流式推送；返回结果（ok 可能为 false，由调用方决定升级）。
fn run_step(
    step_id: &str,
    title: &str,
    programs: &[&str],
    argv: &[String],
    cwd: &Path,
    envs: &[(&str, String)],
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<RunOutcome, String> {
    let _ = channel.send(PipelineEvent::StepStarted {
        id: step_id.into(),
        title: title.into(),
    });
    // 日志按界面语言输出（步骤标题本地化，未知步骤回退原文）。
    // 起止标记仅落盘：步骤进度已由前端 StepStarted/StepFinished 渲染为「▶/✓ 标题」，
    // 不再同时推送 UI，避免日志面板出现重复步骤行（见 logging::file_only_info）。
    let title = crate::i18n::t_or(&format!("step.{step_id}"), title);
    logger.file_only_info(&crate::i18n::t_fmt(
        "log.step_start",
        &[&title, programs[0], &argv.join(" ")],
    ));
    let outcome = plugin::run_capture(programs, argv, cwd, envs, step_id, channel, logger, true, "")?;
    let _ = channel.send(PipelineEvent::StepFinished {
        id: step_id.into(),
        exit_code: outcome.exit_code,
    });
    if outcome.ok {
        logger.file_only_info(&crate::i18n::t_fmt("log.repair_step_done", &[&title]));
    } else {
        logger.error(&crate::i18n::t_fmt(
            "log.repair_step_failed",
            &[&title, &outcome.exit_code.to_string()],
        ));
    }
    Ok(outcome)
}

// ─────────────────────────────── L2 / L3 重建 ───────────────────────────────

/// 完整重建：fetch → reset --hard → clean → install（含构建拦截自动放行）→ build。
/// git 操作由 gitops/libgit2 完成；成功返回 Ok(())；失败返回 Err(描述)（供上层升级处理）。
fn try_rebuild(
    dir: &Path,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    // fetch 必须最先执行：origin/HEAD 可能因中断而陈旧。
    let _ = channel.send(PipelineEvent::StepStarted {
        id: "repair-fetch".into(),
        title: crate::i18n::t("step.repair-fetch"),
    });
    let fetch = crate::gitops::fetch(dir).map_err(|e| e.friendly());
    let _ = channel.send(PipelineEvent::StepFinished {
        id: "repair-fetch".into(),
        exit_code: if fetch.is_ok() { 0 } else { -1 },
    });
    if fetch.is_err() {
        return Err(crate::i18n::t("repair.fetch.failed"));
    }
    let branch = crate::version::default_branch(dir).map_err(|e| e.friendly())?;
    // reset --hard origin/<默认分支>
    let _ = channel.send(PipelineEvent::StepStarted {
        id: "repair-reset".into(),
        title: crate::i18n::t("step.repair-reset"),
    });
    let reset = crate::gitops::reset_hard(dir, &format!("origin/{branch}")).map_err(|e| e.friendly());
    let _ = channel.send(PipelineEvent::StepFinished {
        id: "repair-reset".into(),
        exit_code: if reset.is_ok() { 0 } else { -1 },
    });
    if reset.is_err() {
        return Err(crate::i18n::t("repair.reset.failed"));
    }
    // clean -fdx（保留 node_modules / .venv / .git）
    let _ = channel.send(PipelineEvent::StepStarted {
        id: "repair-clean".into(),
        title: crate::i18n::t("step.repair-clean"),
    });
    let clean = crate::gitops::clean(dir).map_err(|e| e.friendly());
    let _ = channel.send(PipelineEvent::StepFinished {
        id: "repair-clean".into(),
        exit_code: if clean.is_ok() { 0 } else { -1 },
    });
    if clean.is_err() {
        return Err(crate::i18n::t("repair.clean.failed"));
    }
    install_and_build(dir, channel, logger)
}

/// pnpm install（构建脚本被拦截时自动放行重试一次）+ pnpm run build。
fn install_and_build(
    dir: &Path,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    // 运行环境（node/pnpm）就绪，供 pnpm 命令使用。
    crate::runtime::ensure_runtime(channel, logger)?;
    let mut install = run_step(
        "repair-install",
        "安装依赖（pnpm install）",
        &["pnpm.cmd", "pnpm"],
        &["install".into()],
        dir,
        &[],
        channel,
        logger,
    )?;
    // pnpm 拦截依赖构建脚本时，显式放行后重试一次（与插件管理的处理一致，
    // 兼容 git 托管依赖的 ERR_PNPM_GIT_DEP_PREPARE_NOT_ALLOWED）。
    if !install.ok && plugin::is_build_blocked(&install.output) {
        let packages = plugin::parse_blocked_packages(&install.output);
        if packages.is_empty() {
            logger.warn(&crate::i18n::t("log.repair_builds_parse"));
        } else {
            let names: Vec<&str> = packages.iter().map(String::as_str).collect();
            match plugin::ensure_allow_builds(dir, &names) {
                Ok(true) => {
                    logger.warn(&crate::i18n::t_fmt(
                        "log.repair_builds_note1",
                        &[&packages.join("、")],
                    ));
                    install = run_step(
                        "repair-install",
                        "重试安装依赖（pnpm install）",
                        &["pnpm.cmd", "pnpm"],
                        &["install".into()],
                        dir,
                        &[],
                        channel,
                        logger,
                    )?;
                }
                Ok(false) => logger.warn(&crate::i18n::t("log.repair_builds_note2")),
                Err(e) => logger.warn(&crate::i18n::t_fmt(
                    "log.repair_builds_writefail",
                    &[&e],
                )),
            }
        }
    }
    if !install.ok {
        return Err(crate::i18n::t("repair.install.failed"));
    }
    let build = run_step(
        "repair-build",
        "构建（pnpm run build）",
        &["pnpm.cmd", "pnpm"],
        &["run".into(), "build".into()],
        dir,
        &[],
        channel,
        logger,
    )?;
    if !build.ok {
        return Err(crate::i18n::t("repair.build.failed"));
    }
    Ok(())
}

/// 深度重建：删除 node_modules（含构建缓存）后重新安装依赖并构建。
fn deep_rebuild(
    dir: &Path,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    let _ = channel.send(PipelineEvent::StepStarted {
        id: "repair-clean-nm".into(),
        title: "删除 node_modules（深度重建）".into(),
    });
    logger.warn(&crate::i18n::t("log.repair_deep"));
    let nm = dir.join("node_modules");
    if nm.exists() {
        std::fs::remove_dir_all(&nm)
            .map_err(|e| crate::i18n::t_fmt("repair.nm.delete.failed", &[&e.to_string()]))?;
    }
    let _ = channel.send(PipelineEvent::StepFinished {
        id: "repair-clean-nm".into(),
        exit_code: 0,
    });
    install_and_build(dir, channel, logger)
}

// ─────────────────────────────── L4 profile 修复 ───────────────────────────────

/// 修复 profile：对 ~/.dsh/profiles/* 下已初始化的 profile 执行依赖安装（best-effort）。
/// 通过 `dsh plugin --profile <p> install` 走标准通道（自动处理 pnpm dsh 回退与构建拦截）。
fn repair_profiles(app: &AppHandle, channel: &Channel<PipelineEvent>, logger: &Logger) {
    let home = PathBuf::from(crate::detect::dsh_home());
    let profiles = home.join("profiles");
    let Ok(rd) = std::fs::read_dir(&profiles) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_dir() || !p.join("package.json").is_file() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        logger.info(&crate::i18n::t_fmt("log.repair_profile_fix", &[&name]));
        match plugin::run_plugin_op(
            app,
            &name,
            &["install".to_string()],
            "plugin.op.repair",
            &format!("profile {name}"),
            channel,
            logger,
            plugin::PluginOpOpts {
                emit_output: true,
                report_main: false,
                ..Default::default()
            },
        ) {
            Ok(_) => logger.info(&crate::i18n::t_fmt("log.repair_profile_ok", &[&name])),
            Err(e) => logger.warn(&crate::i18n::t_fmt(
                "log.repair_profile_fail",
                &[&name, &e],
            )),
        }
    }
}

// ─────────────────────────────── L5 保底重装 ───────────────────────────────

/// 保底重装：删除安装目录并重新克隆、安装依赖、构建（相当于自动化的卸载 + 重装）。
fn reinstall(
    dir: &str,
    path: &Path,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    let _ = channel.send(PipelineEvent::StepStarted {
        id: "repair-remove".into(),
        title: "删除损坏的安装目录（保底重装）".into(),
    });
    logger.warn(&crate::i18n::t_fmt("log.repair_last_resort", &[dir]));
    if path.exists() {
        std::fs::remove_dir_all(path).map_err(|e| {
            crate::i18n::t_fmt("repair.dir.delete.failed", &[&e.to_string()])
        })?;
    }
    let _ = channel.send(PipelineEvent::StepFinished {
        id: "repair-remove".into(),
        exit_code: 0,
    });
    let url = config::repo_url();
    // 确保运行环境就绪后再克隆（克隆后 pnpm install/build 需要）。
    crate::runtime::ensure_runtime(channel, logger)?;
    let _ = channel.send(PipelineEvent::StepStarted {
        id: "repair-clone".into(),
        title: crate::i18n::t("step.repair-clone"),
    });
    let clone = crate::gitops::clone_with_progress(&url, path, channel, "clone").map_err(|e| e.friendly());
    let _ = channel.send(PipelineEvent::StepFinished {
        id: "repair-clone".into(),
        exit_code: if clone.is_ok() { 0 } else { -1 },
    });
    if clone.is_err() {
        return Err(crate::i18n::t("repair.clone.failed"));
    }
    install_and_build(path, channel, logger)
}

// ─────────────────────────────── 入口 ───────────────────────────────

/// 修复安装主流程（按安装方式分派，见模块文档）。`kernel_id` 指定要修复的内核；
/// `None` 时修复当前活动内核。修复前将该内核设为活动内核（profile 依赖修复据此取目录）。
pub fn repair(
    app: &AppHandle,
    kernel_id: Option<String>,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    let mut cfg = config::load_config(app);
    let (mode, dir) = match kernel_id.as_deref() {
        Some(id) => {
            let k = config::find_kernel(&cfg, id)
                .ok_or_else(|| AppError::NotInstalled.friendly())?;
            (k.mode.clone(), k.install_dir.clone())
        }
        None => (
            cfg.install_mode.clone(),
            cfg
                .install_dir
                .clone()
                .ok_or_else(|| AppError::NotInstalled.friendly())?,
        ),
    };
    // 设为活动内核，使 profile 依赖修复等按被修复内核的目录执行。
    if let Some(id) = kernel_id.as_deref() {
        config::set_active_kernel(&mut cfg, id);
        let _ = config::save_config(app, &cfg);
    }
    let path = PathBuf::from(&dir);

    if mode == "source" {
        if !is_valid_repo(&path) {
            return Err(AppError::NotInstalled.friendly());
        }
        repair_source(app, &dir, &path, channel, logger)?;
    } else {
        if !crate::detect::is_valid_prebuilt(&path) {
            return Err(AppError::NotInstalled.friendly());
        }
        repair_prebuilt(app, &dir, &path, kernel_id.as_deref(), channel, logger)?;
    }

    // L4：profile 依赖修复（best-effort，不阻断主流程；两模式共用）。
    repair_profiles(app, channel, logger);

    // 收尾：更新配置（保持被修复内核为活动内核）。
    let mut cfg = config::load_config(app);
    if mode == "source" {
        cfg.installed_version = crate::detect::read_version(&path);
        cfg.installed_commit = crate::version::read_commit(&path);
    } else {
        cfg.install_dir = Some(dir.clone());
        cfg.installed_version = crate::detect::read_pkg_version(&path);
        cfg.installed_commit = None;
    }
    if let Some(id) = kernel_id.as_deref() {
        cfg.last_started_kernel_id = Some(id.to_string());
    }
    cfg.last_updated_at = Some(config::now_string());
    cfg.update_available = false;
    cfg.latest_commit = None;
    cfg.latest_subject = None;
    config::save_config(app, &cfg).map_err(|e| e)?;

    let _ = channel.send(PipelineEvent::Finished { ok: true });
    logger.info(&crate::i18n::t("log.repair_done"));
    Ok(())
}

/// 源码模式修复：前置校验（网络）→ L1 清理 → L2/L3 重建 → L5 保底重装。
/// git 由 gitops/libgit2 完成，无需外部 Git 环境。
fn repair_source(
    app: &AppHandle,
    dir: &str,
    path: &PathBuf,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    // L0：前置校验（网络可达性）。
    crate::net::ensure_repo_reachable().map_err(|e| e.friendly())?;

    // L1：清理异常状态。
    logger.info(&crate::i18n::t("log.repair_start"));
    let _ = crate::web::stop_web(app);
    kill_stray_processes(dir, logger);
    let locks = git_lock_files(path);
    if !locks.is_empty() {
        for l in &locks {
            let _ = if l.is_dir() {
                std::fs::remove_dir_all(l)
            } else {
                std::fs::remove_file(l)
            };
        }
        logger.info(&crate::i18n::t_fmt(
            "log.repair_locks",
            &[&locks.len().to_string()],
        ));
    } else {
        logger.info(&crate::i18n::t("log.repair_no_locks"));
    }

    // L2 → L3 重建（失败逐级升级）。
    let rebuilt = if try_rebuild(path, channel, logger).is_ok() {
        true
    } else {
        logger.warn(&crate::i18n::t("log.repair_escalate"));
        deep_rebuild(path, channel, logger).is_ok()
    };

    // L5 保底重装（前两级都失败才触发）。
    if !rebuilt {
        logger.error(&crate::i18n::t("log.repair_last_resort_escalate"));
        reinstall(dir, path, channel, logger)?;
    }

    Ok(())
}

/// 预构建模式修复：停 web → 清理残留进程 → 重新下载并解压【该版本】内核到其独立目录。
/// `kernel_id` 为被修复内核的 id（据此定位版本与目录）；`dir`/`path` 为该内核安装目录。
fn repair_prebuilt(
    app: &AppHandle,
    dir: &str,
    path: &Path,
    kernel_id: Option<&str>,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    logger.info(&crate::i18n::t("log.repair_start"));
    let _ = crate::web::stop_web(app);
    kill_stray_processes(dir, logger);

    // 确定要重新下载的版本：优先按内核记录版本；无法识别则按安装目录推导。
    let version = kernel_id
        .and_then(|id| {
            let cfg = config::load_config(app);
            config::find_kernel(&cfg, id).map(|k| k.version)
        })
        .or_else(|| crate::detect::read_pkg_version(path))
        .unwrap_or_else(|| "unknown".to_string());
    let release = crate::prebuilt::release_by_version(&version).map_err(|e| e.friendly())?;

    let _ = channel.send(PipelineEvent::StepStarted {
        id: "download".into(),
        title: crate::i18n::t("step.download"),
    });
    let tmp = std::env::temp_dir().join("dsh-prebuilt-repair.zip");
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
    crate::prebuilt::extract_zip_with_progress(&tmp, Path::new(dir), channel, "extract")
        .map_err(|e| e.friendly())?;
    let _ = channel.send(PipelineEvent::StepFinished {
        id: "extract".into(),
        exit_code: 0,
    });

    // 解压完整性校验：关键入口缺失说明解压不完整，删除损坏目录并报错。
    let dest = PathBuf::from(dir);
    if let Ok(root) = crate::prebuilt::locate_dsh_root(&dest) {
        if let Err(e) = crate::prebuilt::verify_prebuilt_root(&root) {
            let _ = std::fs::remove_dir_all(&dest);
            return Err(e.friendly());
        }
        // profile bundle 校准：移除其它发行版残留的、当前内核无法解析的 bundle。
        for (profile, removed) in crate::plugin::reconcile_all_profiles(&root) {
            if !removed.is_empty() {
                logger.warn(&crate::i18n::t_fmt(
                    "log.profile_reconcile",
                    &[&profile, &removed.join("、")],
                ));
            }
        }
    }

    Ok(())
}

// ─────────────────────────────── 测试 ───────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_repo(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("dsh-repair-{name}-{}", std::process::id()));
        std::fs::create_dir_all(base.join(".git/refs/heads")).unwrap();
        std::fs::create_dir_all(base.join(".git/hooks")).unwrap();
        base
    }

    #[test]
    fn git_lock_files_finds_locks_and_state_files() {
        let repo = tmp_repo("locks");
        std::fs::write(repo.join(".git/index.lock"), "").unwrap();
        std::fs::write(repo.join(".git/MERGE_HEAD"), "").unwrap();
        std::fs::write(repo.join(".git/refs/heads/feature-x.lock"), "").unwrap();
        std::fs::create_dir_all(repo.join(".git/rebase-merge")).unwrap();
        // 正常文件不应被收集。
        std::fs::write(repo.join(".git/config"), "[core]\n").unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();

        let locks = git_lock_files(&repo);
        let names: Vec<String> = locks
            .iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
        assert!(names.iter().any(|n| n.ends_with(".git/index.lock")));
        assert!(names.iter().any(|n| n.ends_with(".git/MERGE_HEAD")));
        assert!(names.iter().any(|n| n.ends_with(".git/refs/heads/feature-x.lock")));
        assert!(names.iter().any(|n| n.ends_with(".git/rebase-merge")));
        assert!(!names.iter().any(|n| n.ends_with(".git/config")));
        assert!(!names.iter().any(|n| n.ends_with(".git/HEAD")));

        std::fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn git_lock_files_empty_when_no_git_dir() {
        let base = std::env::temp_dir().join(format!("dsh-repair-nogit-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        assert!(git_lock_files(&base).is_empty());
        std::fs::remove_dir_all(&base).ok();
    }
}
