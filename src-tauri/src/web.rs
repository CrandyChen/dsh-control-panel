//! web 服务启停：`pnpm dsh web` 的进程管理与端口探测。

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};

use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager};

use crate::config::{self, WEB_PORT};
use crate::detect::{is_valid_prebuilt, is_valid_repo};
use crate::error::AppError;
use crate::logging::Logger;
use crate::process::{spawn_any, PipelineEvent};
use crate::AppState;

/// 快速退出判定窗口（秒）：启动后这段时间内非零退出视为「启动即失败」，
/// 上报可读错误（含输出末尾），而不是静默置为已停止。
const QUICK_EXIT_WINDOW_SECS: u64 = 5;
/// 快速退出时附加到错误里的输出行数上限。
const QUICK_EXIT_TAIL_LINES: usize = 30;

/// 探测 127.0.0.1:port 是否已有服务监听。
pub fn port_in_use(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(300),
    )
    .is_ok()
}

/// 启动 `pnpm dsh web`；输出流式推送，并轮询端口直到就绪/失败。
pub fn start_web(
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
    } else if !is_valid_prebuilt(&path) {
        return Err(AppError::NotInstalled.friendly());
    }
    if port_in_use(WEB_PORT) {
        return Err(AppError::PortInUse(WEB_PORT).friendly());
    }

    // 运行环境（node/pnpm）就绪，供 dsh web 命令使用（首次会下载）。
    crate::runtime::ensure_runtime(channel, logger)?;

    let step_id = if mode == "source" { "web" } else { "web-prebuilt" };
    let step_title = if mode == "source" {
        crate::i18n::t("step.web")
    } else {
        crate::i18n::t("step.web.prebuilt")
    };
    let _ = channel.send(PipelineEvent::StepStarted {
        id: step_id.into(),
        title: step_title,
    });
    let _ = app.emit("web-status", "starting");
    if mode == "source" {
        logger.info(&crate::i18n::t("log.web_start"));
    } else {
        logger.info(&crate::i18n::t("log.web_start_prebuilt"));
    }

    // 预构建模式：启动前校准 profile bundle（幂等）。`~/.dsh` 可能残留其它
    // 发行版写入的、当前内核无法解析的 bundle（如 dsh-tauri），不处理会导致
    // `dsh web` 启动即失败。校准只移除无法解析的条目，不触碰其它配置。
    if mode != "source" {
        for (profile, removed) in crate::plugin::reconcile_all_profiles(&path) {
            if !removed.is_empty() {
                logger.warn(&crate::i18n::t_fmt(
                    "log.profile_reconcile",
                    &[&profile, &removed.join("、")],
                ));
            }
        }
    }

    // 按安装方式选择启动命令。统一加 `--no-open`：dsh web 默认会在启动后自动
    // 打开系统默认浏览器（openBrowser 默认 true），本面板改为在就绪后按
    // 「打开界面默认方式」设置自行分发（tab = 程序内新标签页 / browser = 系统浏览器），
    // 避免不受控地弹出浏览器。
    let programs: Vec<String>;
    let args: Vec<String>;
    if mode == "source" {
        programs = vec!["pnpm.cmd".into(), "pnpm".into()];
        args = vec!["dsh".into(), "web".into(), "--no-open".into()];
    } else {
        // 预构建内核的 dsh CLI 位于安装目录下的 node_modules\.bin\dsh.cmd。
        let dsh = path.join("node_modules").join(".bin").join("dsh.cmd");
        programs = vec![dsh.to_string_lossy().to_string()];
        args = vec!["web".into(), "--no-open".into()];
    }
    let program_refs: Vec<&str> = programs.iter().map(|s| s.as_str()).collect();
    let mut child = spawn_any(&program_refs, &args, Some(&path), &[])
        .map_err(|e| e.friendly())?;
    let pid = child.id();
    if let Some(state) = app.try_state::<AppState>() {
        *state.web_pid.lock().unwrap() = Some(pid);
    }
    logger.info(&crate::i18n::t_fmt("log.web_pid", &[&pid.to_string()]));

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    // 共享输出缓冲（最近 N 行），供快速退出时附加到错误信息。
    let tail: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));

    let ch = channel.clone();
    let logger2 = logger.clone();
    let tail2 = tail.clone();
    if let Some(out) = stdout {
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let _ = ch.send(PipelineEvent::Output {
                    step: "web".into(),
                    stream: "stdout".into(),
                    line: line.clone(),
                });
                push_tail(&tail2, line.clone());
                logger2.log_dsh(&line);
            }
        });
    }
    let ch = channel.clone();
    let logger2 = logger.clone();
    let tail2 = tail.clone();
    if let Some(err) = stderr {
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                let _ = ch.send(PipelineEvent::Output {
                    step: "web".into(),
                    stream: "stderr".into(),
                    line: line.clone(),
                });
                push_tail(&tail2, line.clone());
                logger2.warn(&format!("[DSH] {line}"));
            }
        });
    }

    // 状态监视：HTTP 就绪（页面可响应）→ ready；进程退出 → stopped；
    // 启动后短期内非零退出 → error（附输出末尾，便于定位启动即失败的原因）；
    // 超时 120s → error。
    let app2 = app.clone();
    let channel2 = channel.clone();
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let deadline = started + std::time::Duration::from_secs(120);
        loop {
            if http_ready(WEB_PORT) {
                let _ = app2.emit("web-status", "ready");
                return;
            }
            if let Ok(Some(status)) = child.try_wait() {
                let elapsed = started.elapsed().as_secs();
                let quick_exit = !status.success() && elapsed <= QUICK_EXIT_WINDOW_SECS;
                if quick_exit {
                    let tail_text = {
                        let guard = tail.lock().unwrap();
                        guard.iter().cloned().collect::<Vec<_>>().join("\n")
                    };
                    let msg = crate::i18n::t_fmt(
                        "web.quick_exit",
                        &[&status.code().unwrap_or(-1).to_string()],
                    );
                    let full = if tail_text.trim().is_empty() {
                        msg
                    } else {
                        format!("{msg}\n\n{tail_text}")
                    };
                    let _ = channel2.send(PipelineEvent::Error { message: full.clone() });
                    let _ = app2.emit("web-status", "error");
                } else {
                    let _ = app2.emit("web-status", "stopped");
                }
                return;
            }
            if std::time::Instant::now() > deadline {
                let _ = app2.emit("web-status", "error");
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    });

    Ok(())
}

/// 追加一行到输出缓冲（只保留最近 N 行）。
fn push_tail(tail: &Mutex<VecDeque<String>>, line: String) {
    if let Ok(mut guard) = tail.lock() {
        guard.push_back(line);
        while guard.len() > QUICK_EXIT_TAIL_LINES {
            guard.pop_front();
        }
    }
}

/// 判断 HTTP 响应头是否为 2xx（用于确认页面真正可响应，而非仅端口可连）。
fn is_http_ok(head: &[u8]) -> bool {
    let s = String::from_utf8_lossy(head);
    s.starts_with("HTTP/1.1 2") || s.starts_with("HTTP/1.0 2")
}

/// HTTP 就绪探测：向 127.0.0.1:port 发送 GET /，收到 2xx 响应才算就绪。
fn http_ready(port: u16) -> bool {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let Ok(mut stream) = TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(500),
    ) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(1500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(1500)));
    if stream
        .write_all(b"GET / HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut buf = [0u8; 128];
    let n = stream.read(&mut buf).unwrap_or(0);
    is_http_ok(&buf[..n])
}

/// 从 `netstat -ano` 输出中解析监听 3080 端口的 PID。
pub fn parse_listener_pids(output: &str) -> Vec<u32> {
    let mut pids = Vec::new();
    for line in output.lines() {
        let l = line.to_lowercase();
        if l.contains(":3080") && l.contains("listening") {
            if let Some(t) = l.split_whitespace().last() {
                if let Ok(pid) = t.parse::<u32>() {
                    if !pids.contains(&pid) {
                        pids.push(pid);
                    }
                }
            }
        }
    }
    pids
}

/// 停止 web 服务：先杀记录在案的进程树，再按端口查找残留进程杀掉。
pub fn stop_web(app: &AppHandle) -> Result<(), String> {
    let mut killed: Vec<u32> = Vec::new();

    if let Some(state) = app.try_state::<AppState>() {
        let mut guard = state.web_pid.lock().unwrap();
        if let Some(pid) = *guard {
            let _ = crate::process::no_window(
                Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]),
            )
            .output();
            killed.push(pid);
            *guard = None;
        }
    }

    if let Ok(out) = crate::process::no_window(Command::new("netstat").arg("-ano")).output() {
        let text = String::from_utf8_lossy(&out.stdout);
        for pid in parse_listener_pids(&text) {
            if !killed.contains(&pid) {
                let _ = crate::process::no_window(
                    Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]),
                )
                .output();
                killed.push(pid);
            }
        }
    }

    let _ = app.emit("web-status", "stopped");
    if let Some(state) = app.try_state::<AppState>() {
        if killed.is_empty() {
            state.logger.info(&crate::i18n::t("log.web_stop_noop"));
        } else {
            state.logger.info(&crate::i18n::t_fmt(
                "log.web_stop_killed",
                &[&killed
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")],
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listener_pid_parsing() {
        let sample = "\
  TCP    0.0.0.0:3080           0.0.0.0:0              LISTENING       12345
  TCP    [::]:3080              [::]:0                 LISTENING       12345
  TCP    127.0.0.1:3080         127.0.0.1:51234         ESTABLISHED     12345
  TCP    0.0.0.0:445            0.0.0.0:0              LISTENING       4
";
        assert_eq!(parse_listener_pids(sample), vec![12345]);
    }

    #[test]
    fn http_ok_detection() {
        assert!(is_http_ok(b"HTTP/1.1 200 OK\r\nContent-Type: text/html"));
        assert!(is_http_ok(b"HTTP/1.0 204 No Content"));
        assert!(is_http_ok(b"HTTP/1.1 299 Almost Fine"));
        assert!(!is_http_ok(b"HTTP/1.1 301 Moved Permanently"));
        assert!(!is_http_ok(b"HTTP/1.1 404 Not Found"));
        assert!(!is_http_ok(b"HTTP/1.1 500 Internal Server Error"));
        assert!(!is_http_ok(b""));
        assert!(!is_http_ok(b"garbage"));
    }
}
