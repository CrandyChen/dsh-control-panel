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
/// `kernel_id` 指定启动的内核版本（注册表中的 id）；`None` 时启动当前活动内核。
pub fn start_web(
    app: &AppHandle,
    kernel_id: Option<String>,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    let cfg = config::load_config(app);
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
    // 选定内核后将其设为活动内核（同步镜像字段与「上次启动」记录）。
    if let Some(id) = kernel_id.as_deref() {
        let mut cfg2 = config::load_config(app);
        config::set_active_kernel(&mut cfg2, id);
        let _ = config::save_config(app, &cfg2);
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
        logger.file_only_info(&crate::i18n::t("log.web_start"));
    } else {
        logger.file_only_info(&crate::i18n::t("log.web_start_prebuilt"));
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
        // 新一次启动：清空上一进程捕获的访问 URL（可能已失效）。
        *state.web_url.lock().unwrap() = None;
    }
    logger.info(&crate::i18n::t_fmt("log.web_pid", &[&pid.to_string()]));

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    // 共享输出缓冲（最近 N 行），供快速退出时附加到错误信息。
    let tail: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));

    let ch = channel.clone();
    let logger2 = logger.clone();
    let tail2 = tail.clone();
    let app3 = app.clone();
    if let Some(out) = stdout {
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                // 捕获 DSH 输出的带 token 访问 URL（新版内核；旧内核无则保持 None）。
                // 该 URL 含进程级 token，仅经内存写入状态供打开界面使用，不写日志。
                let captured = extract_token_url(&line);
                if let Some(url) = captured.clone() {
                    if let Some(state) = app3.try_state::<AppState>() {
                        *state.web_url.lock().unwrap() = Some(url);
                    }
                }
                // token 行既不推送 UI（PipelineEvent::Output）、不落日志，也不进快速退出
                // 错误尾部，避免 token 泄漏到日志面板/日志文件/错误信息。
                if captured.is_none() {
                    let _ = ch.send(PipelineEvent::Output {
                        step: "web".into(),
                        stream: "stdout".into(),
                        line: line.clone(),
                    });
                    push_tail(&tail2, line.clone());
                    logger2.log_dsh(&line);
                }
            }
        });
    }
    let ch = channel.clone();
    let logger2 = logger.clone();
    let tail2 = tail.clone();
    if let Some(err) = stderr {
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                // 防御性过滤：极少数情况下 token 地址可能出现在 stderr，同样不推送 UI/不落盘。
                if extract_token_url(&line).is_some() {
                    continue;
                }
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
    let logger3 = logger.clone();
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let deadline = started + std::time::Duration::from_secs(120);
        // 是否需要等待 DSH 打印出带 token 的访问地址（新内核认证）。
        let mut awaiting_token = false;
        loop {
            if !awaiting_token {
                match http_probe(WEB_PORT) {
                    HttpProbe::NotReady => {}
                    HttpProbe::Ready => {
                        // 旧内核直接可用：无需 token。
                        logger3.info(&crate::i18n::t("log.web_ready"));
                        let _ = app2.emit("web-status", "ready");
                        return;
                    }
                    HttpProbe::ReadyAuth => {
                        // 新内核要求浏览器会话认证。`dsh-web-app` 在服务就绪后会打印
                        // `dsh web: …?token=…`（进程级会话令牌，非持久化）。必须等它被
                        // stdout 线程捕获后再发 ready，否则 openDshTab / open_web_ui 会
                        // 回退到不带 token 的默认地址（导致 401）。
                        awaiting_token = true;
                    }
                }
            } else {
                let has_token = app2
                    .try_state::<AppState>()
                    .map(|s| s.web_url.lock().unwrap().is_some())
                    .unwrap_or(false);
                if has_token {
                    logger3.info(&crate::i18n::t("log.web_ready"));
                    let _ = app2.emit("web-status", "ready");
                    return;
                }
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
            std::thread::sleep(std::time::Duration::from_millis(200));
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

/// 从 DSH web 进程输出行中提取带 token 的访问 URL。
///
/// 新版内核的 `dsh-web-app` 会打印 `dsh web: <authenticatedUrl> (LAN: …)`，
/// 其中 `<authenticatedUrl>` 为 `http://127.0.0.1:<port>/?token=<…>`，token 是进程级的
/// 浏览器会话启动令牌（每进程动态生成，不持久化）。面板必须捕获它才能在系统浏览器或
/// 内嵌 iframe 里正常打开界面（否则 `GET /` 返回 401）。只接受 loopback 主机地址
/// （127.x / localhost），返回捕获到的完整 URL（含 token）；无匹配返回 None。
fn extract_token_url(line: &str) -> Option<String> {
    // 先定位 token 参数（可能是 `?token=` 或前面的 `&token=`）。
    let (token_pos, token_assignment_len) = {
        let p = line.find("?token=");
        match p {
            Some(p) => (p, "?token=".len()),
            None => {
                let p = line.find("&token=")?;
                (p, "&token=".len())
            }
        }
    };
    // 找到同一行里 URL 的起点（最近的一个 http:// 或 https://）。
    let up_to = &line[..token_pos];
    let url_start = up_to
        .rfind("http://")
        .or_else(|| up_to.rfind("https://"))?;
    // token 值到下一个空白/控制字符为止（base64url 不含空白；按行尾或空格截断）。
    let rest = &line[token_pos + token_assignment_len..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c.is_control())
        .map(|i| token_pos + token_assignment_len + i)
        .unwrap_or(line.len());
    let candidate = &line[url_start..end];
    if end > url_start && is_loopback_url(candidate) {
        Some(candidate.to_string())
    } else {
        None
    }
}

/// 判断 URL 是否指向 loopback 主机（127.x / localhost），即面板自身的 DSH web 实例。
fn is_loopback_url(url: &str) -> bool {
    let Some(after_scheme) = url.split("://").nth(1) else {
        return false;
    };
    let authority = after_scheme.split('/').next().unwrap_or(after_scheme);
    let host = authority.split(':').next().unwrap_or(authority);
    host.eq_ignore_ascii_case("localhost") || host.starts_with("127.")
}

/// 就绪探测结果分类。
#[derive(Debug, Clone, Copy, PartialEq)]
enum HttpProbe {
    /// 端口未就绪或响应异常。
    NotReady,
    /// 服务已就绪（返回 2xx，旧内核），无需 token 即可访问。
    Ready,
    /// 服务已就绪但要求浏览器会话认证（3xx/4xx 等，如新内核的 401/303）——
    /// 需携带 `?token=` 才能正常打开界面。
    ReadyAuth,
}

/// 判断 HTTP 响应头是否为一个合法状态行（服务可响应），并区分 2xx 与认证需求。
fn classify_http_header(head: &[u8]) -> HttpProbe {
    let s = String::from_utf8_lossy(head);
    if s.starts_with("HTTP/1.1 2") || s.starts_with("HTTP/1.0 2") {
        return HttpProbe::Ready;
    }
    if s.starts_with("HTTP/1.1 ") || s.starts_with("HTTP/1.0 ") {
        return HttpProbe::ReadyAuth;
    }
    HttpProbe::NotReady
}

/// HTTP 就绪探测：向 127.0.0.1:port 发送 GET /，收到任何合法 HTTP 状态行即视为就绪，
/// 并区分是否为「要求认证」的新内核（旧内核 200；新内核 401/303）。
fn http_probe(port: u16) -> HttpProbe {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let Ok(mut stream) = TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(500),
    ) else {
        return HttpProbe::NotReady;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(1500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(1500)));
    if stream
        .write_all(b"GET / HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n")
        .is_err()
    {
        return HttpProbe::NotReady;
    }
    let mut buf = [0u8; 128];
    let n = stream.read(&mut buf).unwrap_or(0);
    classify_http_header(&buf[..n])
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
    fn http_probe_classification() {
        // 2xx（旧内核）→ 可直接就绪，无需 token。
        assert_eq!(
            classify_http_header(b"HTTP/1.1 200 OK\r\nContent-Type: text/html"),
            HttpProbe::Ready
        );
        assert_eq!(classify_http_header(b"HTTP/1.0 204 No Content"), HttpProbe::Ready);
        assert_eq!(classify_http_header(b"HTTP/1.1 299 Almost Fine"), HttpProbe::Ready);
        // 401/303 等（新内核认证）→ 就绪但需 token。
        assert_eq!(
            classify_http_header(b"HTTP/1.1 401 Unauthorized\r\ncontent-type: text/plain"),
            HttpProbe::ReadyAuth
        );
        assert_eq!(
            classify_http_header(b"HTTP/1.0 303 See Other\r\nlocation: /"),
            HttpProbe::ReadyAuth
        );
        assert_eq!(classify_http_header(b"HTTP/1.1 404 Not Found"), HttpProbe::ReadyAuth);
        assert_eq!(
            classify_http_header(b"HTTP/1.1 500 Internal Server Error"),
            HttpProbe::ReadyAuth
        );
        // 无 / 乱响应 → 未就绪。
        assert_eq!(classify_http_header(b""), HttpProbe::NotReady);
        assert_eq!(classify_http_header(b"garbage"), HttpProbe::NotReady);
        assert_eq!(classify_http_header(b"HTTP/2 200 OK"), HttpProbe::NotReady);
    }

    #[test]
    fn token_url_extraction() {
        assert_eq!(
            extract_token_url("dsh web: http://127.0.0.1:3080/?token=abc-DEF_123"),
            Some("http://127.0.0.1:3080/?token=abc-DEF_123".to_string())
        );
        // 行首留有空白亦能提取。
        assert_eq!(
            extract_token_url("  http://127.0.0.1:3080/?token=tok1"),
            Some("http://127.0.0.1:3080/?token=tok1".to_string())
        );
        // 无 token 的行返回 None。
        assert_eq!(extract_token_url("dsh web: http://127.0.0.1:3080"), None);
        assert_eq!(extract_token_url("some unrelated output"), None);
        assert_eq!(extract_token_url(""), None);
    }

    #[test]
    fn token_url_extraction_ignores_other_hosts() {
        // 非本机（非 loopback）的 token 地址不误捕获。
        assert_eq!(extract_token_url("https://example.com/?token=x"), None);
        // 真实输出行可能有前后缀文本，仍能提取本机带 token 的地址。
        assert_eq!(
            extract_token_url("dsh web: http://127.0.0.1:3080/?token=y (pid 123)"),
            Some("http://127.0.0.1:3080/?token=y".to_string())
        );
        // 含 LAN 后缀时只取 loopback 的本机地址。
        assert_eq!(
            extract_token_url(
                "dsh web: http://127.0.0.1:3080/?token=a (LAN: http://192.168.1.5:3080/?token=b)"
            ),
            Some("http://127.0.0.1:3080/?token=a".to_string())
        );
        // localhost 亦接受。
        assert_eq!(
            extract_token_url("dsh web: http://localhost:3080/?token=c"),
            Some("http://localhost:3080/?token=c".to_string())
        );
    }
}
