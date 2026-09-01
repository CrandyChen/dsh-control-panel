//! 子进程执行器：按步骤串行执行命令，stdout/stderr 逐行流式推送到前端 Channel。
//!
//! 用于安装 / 更新等长任务流水线。程序名支持 .cmd/.exe 变体探测（pnpm 在
//! Windows 上实际是 pnpm.cmd，std::process::Command 不会自动解析 PATHEXT）。

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde::Serialize;

use crate::error::AppError;
use crate::logging::Logger;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Windows：不创建新控制台窗口（GUI 进程 spawn 控制台子进程时默认会弹出终端窗口）。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 为命令附加 CREATE_NO_WINDOW：子进程照常通过管道输出，但不弹终端窗口。
/// 终端类操作（打开 PowerShell）不要使用本函数。
pub fn no_window(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// 流水线事件（序列化后 type 为 camelCase：stepStarted/output/stepFinished/error/finished）。
#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PipelineEvent {
    StepStarted { id: String, title: String },
    Output { step: String, stream: String, line: String },
    StepFinished { id: String, exit_code: i32 },
    Error { message: String },
    Finished { ok: bool },
    /// 预构建内核下载进度（received/total 为字节；speed_bps 为滑动窗口平均速度）。
    DownloadProgress {
        step: String,
        received: u64,
        total: u64,
        speed_bps: u64,
    },
    /// 运行环境（node/pnpm）下载/解压完成：前端据此隐藏运行环境进度条。
    RuntimeDone,
}

/// 流水线中的一个步骤。
pub struct Step {
    pub id: &'static str,
    pub title: &'static str,
    pub program: &'static str,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub envs: Vec<(&'static str, String)>,
}

/// 依次尝试多个程序名（如 ["pnpm.cmd", "pnpm"]），以兼容不同安装方式。
/// 子进程统一使用内置运行时 PATH（便携模式），dev 下回退系统 PATH。
///
/// 便携模式还**显式优先内置 pnpm**（把 `runtime\pnpm` 的完整路径放到候选首位），
/// 确保源码安装等场景用自带 pnpm 而非全局安装（目标用户可能未安装 node/pnpm）。
pub fn spawn_any(
    programs: &[&str],
    args: &[String],
    cwd: Option<&PathBuf>,
    envs: &[(&str, String)],
) -> Result<std::process::Child, AppError> {
    let mut candidates: Vec<String> = programs.iter().map(|s| s.to_string()).collect();
    let is_pnpm = candidates.iter().any(|p| p == "pnpm.cmd" || p == "pnpm");
    if is_pnpm {
        if let Some(bundled) = crate::config::bundled_pnpm_path() {
            if !candidates.contains(&bundled) {
                candidates.insert(0, bundled);
            }
        }
    }
    let mut last_err: Option<std::io::Error> = None;
    for prog in &candidates {
        let mut cmd = Command::new(prog);
        no_window(&mut cmd);
        cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
        // 便携模式：把内置运行时目录前置到 PATH，使 node/pnpm/npm/npx/dsh.cmd 可用。
        cmd.env("PATH", crate::config::augmented_path());
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        for (k, v) in envs {
            cmd.env(k, v);
        }
        match cmd.spawn() {
            Ok(c) => return Ok(c),
            Err(e) => last_err = Some(e),
        }
    }
    Err(match last_err {
        Some(e) if e.kind() == std::io::ErrorKind::NotFound => {
            AppError::ProgramNotFound(programs[0].to_string())
        }
        Some(e) => AppError::Io(e.to_string()),
        None => AppError::Io("无法启动进程".to_string()),
    })
}

/// 串行执行步骤流水线；任一失败即中止并返回错误。
pub fn run_pipeline(
    steps: &[Step],
    channel: &tauri::ipc::Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), AppError> {
    for step in steps {
        let _ = channel.send(PipelineEvent::StepStarted {
            id: step.id.into(),
            title: step.title.into(),
        });
        // 日志按界面语言输出（步骤标题本地化，未知步骤回退原文）。
        // 该起止标记仅落盘：步骤进度已由前端 StepStarted/StepFinished 渲染为「▶/✓ 标题」，
        // 不再同时推送 UI，避免日志面板出现重复步骤行（见 logging::file_only_info）。
        let title = crate::i18n::t_or(&format!("step.{}", step.id), step.title);
        logger.file_only_info(&crate::i18n::t_fmt(
            "log.step_start",
            &[&title, step.program, &step.args.join(" ")],
        ));

        let mut child = match spawn_any(&[step.program], &step.args, step.cwd.as_ref(), &step.envs)
        {
            Ok(c) => c,
            Err(e) => {
                let msg = e.friendly();
                let _ = channel.send(PipelineEvent::Error { message: msg.clone() });
                logger.error(&msg);
                return Err(e);
            }
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let ch = channel.clone();
        let sid = step.id.to_string();
        if let Some(out) = stdout {
            std::thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    let _ = ch.send(PipelineEvent::Output {
                        step: sid.clone(),
                        stream: "stdout".into(),
                        line,
                    });
                }
            });
        }

        let ch = channel.clone();
        let sid = step.id.to_string();
        if let Some(err) = stderr {
            std::thread::spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    let _ = ch.send(PipelineEvent::Output {
                        step: sid.clone(),
                        stream: "stderr".into(),
                        line,
                    });
                }
            });
        }

        let status = child.wait()?;
        let exit_code = status.code().unwrap_or(-1);
        logger.file_only_info(&crate::i18n::t_fmt(
            "log.step_done",
            &[&title, &exit_code.to_string()],
        ));
        let _ = channel.send(PipelineEvent::StepFinished {
            id: step.id.into(),
            exit_code,
        });

        if !status.success() {
            // StepFailed 的 step 用本地化标题，保证英文错误文案不混杂中文。
            let err = AppError::StepFailed {
                step: title,
                exit_code,
            };
            let msg = err.friendly();
            let _ = channel.send(PipelineEvent::Error { message: msg.clone() });
            logger.error(&msg);
            return Err(err);
        }
    }

    let _ = channel.send(PipelineEvent::Finished { ok: true });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_event_serializes_with_camel_case_tag() {
        let e = PipelineEvent::StepStarted {
            id: "clone".into(),
            title: "克隆仓库".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"type\":\"stepStarted\""), "{json}");
        assert!(json.contains("\"id\":\"clone\""), "{json}");
    }

    #[test]
    fn spawn_any_missing_program_reports_friendly_error() {
        let err = spawn_any(
            &["definitely-not-a-real-program-xyz"],
            &[],
            None,
            &[],
        )
        .unwrap_err();
        assert!(matches!(err, AppError::ProgramNotFound(_)));
    }
}
