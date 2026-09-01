//! 日志：按天写入 app_log_dir/logs，保留最近 5 份，同时转发前端事件。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

struct LoggerInner {
    file: Option<File>,
}

#[derive(Clone)]
pub struct Logger {
    app: AppHandle,
    inner: Arc<Mutex<LoggerInner>>,
}

impl Logger {
    pub fn init(app: &AppHandle) -> Self {
        let logger = Self {
            app: app.clone(),
            inner: Arc::new(Mutex::new(LoggerInner { file: None })),
        };
        let _ = logger.open_today_file();
        logger.rotate();
        logger.info(&crate::i18n::t_fmt(
            "log.boot",
            &[env!("CARGO_PKG_VERSION")],
        ));
        logger.info(&crate::i18n::t_fmt(
            "log.os",
            &[
                std::env::consts::OS,
                &logger.log_file_path().display().to_string(),
            ],
        ));
        logger
    }

    /// 日志目录：优先 exe 所在目录的 logs/（portable 语义），不可写时回退应用日志目录。
    pub fn log_dir(&self) -> PathBuf {
        let exe_logs = crate::config::exe_dir().join("logs");
        if fs::create_dir_all(&exe_logs).is_ok() {
            exe_logs
        } else {
            self.app
                .path()
                .app_log_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("dsh-control-panel"))
        }
    }

    pub fn log_file_path(&self) -> PathBuf {
        self.log_dir().join(format!("control-panel-{}.log", crate::config::date_string()))
    }

    fn open_today_file(&self) -> Result<(), String> {
        let dir = self.log_dir();
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("control-panel-{}.log", crate::config::date_string()));
        let f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        if let Ok(mut guard) = self.inner.lock() {
            guard.file = Some(f);
        }
        Ok(())
    }

    /// 按天轮转：只保留最新的 5 份 control-panel-*.log（并顺带清理旧版 launcher-*.log）。
    fn rotate(&self) {
        let dir = self.log_dir();
        let Ok(entries) = fs::read_dir(&dir) else {
            return;
        };
        let mut logs: Vec<(std::time::SystemTime, PathBuf)> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                let os_name = e.file_name();
                let n = os_name.to_string_lossy();
                (n.starts_with("control-panel-") || n.starts_with("launcher-")) && n.ends_with(".log")
            })
            .filter_map(|e| {
                e.metadata()
                    .ok()
                    .map(|m| (m.modified().unwrap_or(std::time::UNIX_EPOCH), e.path()))
            })
            .collect();
        logs.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, p) in logs.into_iter().skip(5) {
            let _ = fs::remove_file(p);
        }
    }

    /// 把一行写入当日日志文件（不推送前端事件），返回生成的时间戳。
    fn write_file(&self, level: &str, msg: &str) -> String {
        let ts = crate::config::now_string();
        let line = format!("[{ts}] [{level}] {msg}");
        if let Ok(mut guard) = self.inner.lock() {
            if let Some(f) = guard.file.as_mut() {
                let _ = writeln!(f, "{line}");
                let _ = f.flush();
            }
        }
        ts
    }

    pub fn log(&self, level: &str, msg: &str) {
        let ts = self.write_file(level, msg);
        let _ = self.app.emit(
            "log-line",
            serde_json::json!({ "level": level, "text": msg, "ts": ts }),
        );
    }

    /// 只写入日志文件，不推送前端 `log-line` 事件：
    /// 用于已被前端 `StepStarted`/`StepFinished` 步骤事件展示的起止标记，
    /// 避免同一进度在日志面板重复出现，同时保留完整磁盘日志。
    pub fn file_only(&self, level: &str, msg: &str) {
        let _ = self.write_file(level, msg);
    }

    pub fn file_only_info(&self, msg: &str) {
        self.file_only("INFO", msg);
    }

    pub fn info(&self, msg: &str) {
        self.log("INFO", msg);
    }

    pub fn warn(&self, msg: &str) {
        self.log("WARN", msg);
    }

    pub fn error(&self, msg: &str) {
        self.log("ERROR", msg);
    }

    /// 记录 DeepSeek Harness 进程输出（落盘带 [DSH] 前缀，与控制面板自身行为日志区分）。
    pub fn log_dsh(&self, line: &str) {
        self.log("INFO", &format!("[DSH] {line}"));
    }

    pub fn read_today(&self) -> Vec<String> {
        let path = self.log_dir().join(format!("control-panel-{}.log", crate::config::date_string()));
        fs::read_to_string(&path)
            .map(|s| s.lines().map(|l| l.to_string()).collect())
            .unwrap_or_default()
    }

    pub fn clear_today(&self) {
        let path = self.log_dir().join(format!("control-panel-{}.log", crate::config::date_string()));
        let _ = fs::write(&path, "");
    }
}
