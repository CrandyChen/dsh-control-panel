//! DSH Control Panel — DeepSeek Harness 控制面板（Tauri 2）。
//!
//! 职责：安装 / 更新 / 检测 / 启动 / 卸载 DeepSeek Harness，
//! 只执行标准 git/pnpm 命令，绝不修改 DSH 源码；控制面板自身配置与日志
//! 保存在 exe 所在目录（portable 语义），与 DSH 完全隔离。
//!
//! 多 Tab 浏览器为纯前端 iframe 方案（控制面板主界面常驻挂载），无需原生 webview。

mod config;
mod detect;
mod error;
mod gitops;
mod i18n;
mod install;
mod logging;
mod net;
mod plugin;
mod prebuilt;
mod process;
mod repair;
mod runtime;
mod terminal;
mod tools;
mod uninstall;
mod update;
mod version;
mod web;
mod balance;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, RunEvent};

use config::AppConfig;
use detect::DetectResult;
use logging::Logger;
use process::PipelineEvent;
use uninstall::UninstallPreview;
use version::UpdateCheckResult;

/// 全局状态：web 服务 PID + 日志器 + 卸载预览扫描取消标志 + 插件更新检测结果。
pub struct AppState {
    pub web_pid: Mutex<Option<u32>>,
    pub logger: Logger,
    /// 卸载预览扫描的取消标志（`uninstall_preview` 运行期间存在，取消后清空）。
    pub preview_cancel: Mutex<Option<Arc<AtomicBool>>>,
    /// 最近一次插件更新检测结果（默认 profile，供前端徽标展示）。
    pub plugin_updates: Mutex<Option<plugin::PluginUpdates>>,
}

// ─────────────────────────────── 命令 ───────────────────────────────

#[tauri::command]
fn get_config(app: AppHandle) -> AppConfig {
    config::load_config(&app)
}

#[tauri::command]
fn save_config(app: AppHandle, cfg: AppConfig) -> Result<(), String> {
    let logger = app.state::<AppState>().logger.clone();
    // 同步界面语言到全局（错误提示等后端文案按当前语言输出）。
    i18n::set_lang(i18n::lang_from_config(&cfg.language));
    config::save_config(&app, &cfg)?;
    logger.info(&crate::i18n::t("log.settings_saved"));
    Ok(())
}

/// 源码安装模式的默认父目录（程序运行目录），供前端安装弹窗展示目标路径。
#[tauri::command]
fn default_parent_dir() -> String {
    config::mode1_default_parent().to_string_lossy().to_string()
}

#[tauri::command]
async fn detect_state(app: AppHandle) -> DetectResult {
    tauri::async_runtime::spawn_blocking(move || {
        let logger = app.state::<AppState>().logger.clone();
        let cfg = config::load_config(&app);
        let result = detect::detect_state(cfg.install_dir.as_deref(), &cfg.install_mode);
        // 版本与 commit 均由 detect_state 实时读取（config 中可能因手动 git pull 而陈旧）。
        let commit = result
            .installed_commit
            .as_deref()
            .map(|c| c.chars().take(7).collect::<String>())
            .unwrap_or_else(|| "—".to_string());
        logger.info(&crate::i18n::t_fmt(
            "log.detect_state",
            &[
                &result.installed.to_string(),
                result.version.as_deref().unwrap_or("—"),
                &commit,
                &result.running.to_string(),
            ],
        ));
        result
    })
    .await
    .unwrap_or_else(|_| DetectResult {
        installed: false,
        valid: false,
        built: false,
        version: None,
        running: false,
        install_dir: None,
        dsh_home: detect::dsh_home(),
        installed_commit: None,
    })
}

/// 检测运行环境工具（git 已内置、node/pnpm 按需下载，现无必装项），启动时自动调用。
#[tauri::command]
fn detect_tools(app: AppHandle) -> Vec<tools::ToolStatus> {
    let logger = app.state::<AppState>().logger.clone();
    let cfg = config::load_config(&app);
    let result = tools::detect_tools(&cfg.install_mode);
    let summary = result
        .iter()
        .map(|t| {
            format!(
                "{}={}{}",
                t.id,
                if t.installed { "ok" } else { "missing" },
                t.version.as_deref().map(|v| format!("({v})")).unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    logger.info(&crate::i18n::t_fmt("log.tools_summary", &[&summary]));
    result
}

/// 查询 DeepSeek 当前余额（读 `$DSH_HOME/.credentials.yaml` 的 DEEPSEEK_API_KEY）。
/// 走 spawn_blocking，避免 PowerShell 请求阻塞主线程。
#[tauri::command]
async fn get_balance() -> balance::BalanceResult {
    tauri::async_runtime::spawn_blocking(balance::get_balance)
        .await
        .unwrap_or_else(|_| balance::BalanceResult {
            available: false,
            api_key_set: false,
            is_available: None,
            currency: None,
            balance: None,
            error: Some(crate::i18n::t("balance.no_key")),
        })
}

#[tauri::command]
async fn install(
    app: AppHandle,
    mode: String,
    channel: Channel<PipelineEvent>,
) -> Result<(), String> {
    let logger = app.state::<AppState>().logger.clone();
    tauri::async_runtime::spawn_blocking(move || {
        install::install(&app, &mode, &channel, &logger)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn check_for_updates(app: AppHandle) -> Result<UpdateCheckResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let logger = app.state::<AppState>().logger.clone();
        let cfg = config::load_config(&app);
        let mode = cfg.install_mode.clone();
        let dir = cfg
            .install_dir
            .clone()
            .ok_or_else(|| error::AppError::NotInstalled.friendly())?;
        let path = PathBuf::from(&dir);
        if mode == "source" && !detect::is_valid_repo(&path) {
            return Err(error::AppError::NotInstalled.friendly());
        }
        if mode != "source" && !detect::is_valid_prebuilt(&path) {
            return Err(error::AppError::NotInstalled.friendly());
        }
        // 预构建模式以磁盘实时版本为准：配置中可能残留旧值（如打包外壳版本
        // 0.1.1-rc.1），与 release tag 不一致会造成「永远有新版本」。
        let current_version = if mode == "source" {
            cfg.installed_version.clone()
        } else {
            detect::read_pkg_version(&path).or_else(|| cfg.installed_version.clone())
        };
        let result = version::check_for_updates_in_dir(&path, &mode, current_version.as_deref())
            .map_err(|e| e.friendly())?;
        logger.info(&crate::i18n::t_fmt(
            "log.update_check",
            &[
                &result.behind.to_string(),
                &result.update_available.to_string(),
                &result.subject,
            ],
        ));

        let mut cfg = config::load_config(&app);
        cfg.update_available = result.update_available;
        cfg.latest_commit = Some(result.remote_commit.clone());
        cfg.latest_subject = Some(result.subject.clone());
        cfg.last_check_at = Some(result.checked_at.clone());
        config::save_config(&app, &cfg).map_err(|e| e)?;

        let _ = app.emit("update-checked", &result);
        Ok(result)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn update(app: AppHandle, channel: Channel<PipelineEvent>) -> Result<(), String> {
    let logger = app.state::<AppState>().logger.clone();
    tauri::async_runtime::spawn_blocking(move || update::update(&app, &channel, &logger))
        .await
        .map_err(|e| e.to_string())?
}

/// 生成卸载预览清单（安装目录 + DSH 用户数据目录）。统计通过 channel 实时
/// 推送进度（step="scan" 的 Output 行），扫描可被 `cancel_uninstall_preview` 取消。
#[tauri::command]
async fn uninstall_preview(
    app: AppHandle,
    channel: Channel<PipelineEvent>,
) -> Result<UninstallPreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let state = app.state::<AppState>();
            *state.preview_cancel.lock().unwrap() = Some(cancel.clone());
        }
        let result = uninstall::build_preview(&app, &channel, &cancel);
        {
            let state = app.state::<AppState>();
            *state.preview_cancel.lock().unwrap() = None;
        }
        result
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 取消进行中的卸载预览扫描（没有扫描在运行时为空操作）。
#[tauri::command]
fn cancel_uninstall_preview(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    if let Some(flag) = state.preview_cancel.lock().unwrap().as_ref() {
        flag.store(true, Ordering::Relaxed);
    }
    Ok(())
}

#[tauri::command]
async fn uninstall(
    app: AppHandle,
    selected: Vec<String>,
    channel: Channel<PipelineEvent>,
) -> Result<(), String> {
    let logger = app.state::<AppState>().logger.clone();
    tauri::async_runtime::spawn_blocking(move || {
        uninstall::uninstall(&app, selected, &channel, &logger)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn start_web(app: AppHandle, channel: Channel<PipelineEvent>) -> Result<(), String> {
    let logger = app.state::<AppState>().logger.clone();
    tauri::async_runtime::spawn_blocking(move || web::start_web(&app, &channel, &logger))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn stop_web(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || web::stop_web(&app))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn open_terminal(app: AppHandle) -> Result<(), String> {
    let logger = app.state::<AppState>().logger.clone();
    let cfg = config::load_config(&app);
    let dir = cfg
        .install_dir
        .clone()
        .ok_or_else(|| error::AppError::NotInstalled.friendly())?;
    // 打开安装目录的父目录（便于直接查看 / 操作 dsh、deepseek-harness 等同级内容）；
    // 父目录不存在时回退为安装目录本身。
    let target = std::path::Path::new(&dir)
        .parent()
        .map(|p| p.to_path_buf())
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| std::path::PathBuf::from(&dir));
    let target_str = target.to_string_lossy().to_string();
    terminal::open_terminal(&target_str)?;
    logger.info(&crate::i18n::t_fmt("log.open_terminal", &[&target_str]));
    Ok(())
}

#[tauri::command]
fn open_web_ui(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let logger = app.state::<AppState>().logger.clone();
    app.opener()
        .open_url(config::WEB_URL, None::<&str>)
        .map_err(|e| e.to_string())?;
    logger.info(&crate::i18n::t_fmt("log.open_browser", &[config::WEB_URL]));
    Ok(())
}

/// 在系统浏览器中打开外部链接（安装指引页使用；协议已由前端校验为 http/https）。
#[tauri::command]
fn open_external(app: AppHandle, url: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let logger = app.state::<AppState>().logger.clone();
    app.opener()
        .open_url(&url, None::<&str>)
        .map_err(|e| e.to_string())?;
    logger.info(&crate::i18n::t_fmt("log.open_external", &[&url]));
    Ok(())
}

#[tauri::command]
fn get_logs(app: AppHandle) -> Vec<String> {
    app.state::<AppState>().logger.read_today()
}

#[tauri::command]
fn get_log_dir(app: AppHandle) -> String {
    app.state::<AppState>()
        .logger
        .log_file_path()
        .to_string_lossy()
        .to_string()
}

#[tauri::command]
fn clear_logs(app: AppHandle) -> Result<(), String> {
    app.state::<AppState>().logger.clear_today();
    Ok(())
}

// ─────────────────────────────── 插件管理 ───────────────────────────────

/// 读取指定 profile 的插件列表（dependencies + 内置组合包）。
#[tauri::command]
fn plugin_list(app: AppHandle, profile: String) -> Result<plugin::PluginList, String> {
    let dir = plugin::profile_dir(&profile)?;
    let cfg = config::load_config(&app);
    Ok(plugin::read_plugin_list(&dir, &profile, cfg.use_pnpm_dsh))
}

/// 列出 `$DSH_HOME/profiles` 下已存在的 profile（供插件管理下拉框选择）。
#[tauri::command]
fn plugin_profiles() -> Vec<String> {
    plugin::list_profiles()
}

/// 检测指定 profile 已安装第三方插件的更新（npm 查 registry 最新版，github 查最高 tag）。
#[tauri::command]
async fn plugin_check_updates(app: AppHandle, profile: String) -> Result<plugin::PluginUpdates, String> {
    let cfg = config::load_config(&app);
    let dir = cfg
        .install_dir
        .clone()
        .ok_or_else(|| error::AppError::NotInstalled.friendly())?;
    tauri::async_runtime::spawn_blocking(move || plugin::check_plugin_updates(&profile, &dir))
        .await
        .map_err(|e| e.to_string())?
}

/// 智能解析输入并安装插件（npm 包名 / github 标识 / GitHub 链接 / 完整命令）。
/// 完整命令中若带 --profile 则优先于对话框当前 profile。
#[tauri::command]
async fn plugin_install(
    app: AppHandle,
    input: String,
    profile: String,
    channel: Channel<PipelineEvent>,
) -> Result<plugin::PluginOpResult, String> {
    let logger = app.state::<AppState>().logger.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let parsed = plugin::parse_add_input(&input)?;
        let target = parsed.profile.clone().unwrap_or(profile);
        let subject = parsed.specs.join("、");
        // 必须带 add 动词：dsh plugin --profile X add <spec...>
        let args = plugin::install_args(&parsed.specs);
        plugin::run_plugin_op(&app, &target, &args, "plugin.op.install", &subject, &channel, &logger)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 更新指定插件（specs 为列表中的依赖 key，github 标识需与安装时完全一致；支持批量）。
/// 多个 key 时逐个执行 `dsh plugin update`（全程只有一次 web 服务的启停封装），
/// 结果逐条聚合返回。
#[tauri::command]
async fn plugin_update(
    app: AppHandle,
    specs: Vec<String>,
    profile: String,
    channel: Channel<PipelineEvent>,
) -> Result<plugin::PluginOpResult, String> {
    let logger = app.state::<AppState>().logger.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let cleaned: Vec<String> = specs
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if cleaned.is_empty() {
            return Err(i18n::t("plugin.missing.update.spec"));
        }
        let total = cleaned.len();
        let mut messages: Vec<String> = Vec::new();
        let mut all_ok = true;
        for (idx, spec) in cleaned.iter().enumerate() {
            // 实时进度：经 StepStarted 事件推给前端顶部展示（单个不带编号）。
            let title = if total == 1 {
                i18n::t_fmt("plugin.progress.updating.one", &[spec])
            } else {
                i18n::t_fmt(
                    "plugin.progress.updating",
                    &[&(idx + 1).to_string(), &total.to_string(), spec],
                )
            };
            let _ = channel.send(PipelineEvent::StepStarted {
                id: format!("item-{idx}"),
                title,
            });
            match plugin::run_plugin_op(
                &app,
                &profile,
                &["update".to_string(), spec.clone()],
                "plugin.op.update",
                spec,
                &channel,
                &logger,
            ) {
                Ok(r) => {
                    messages.push(r.message);
                    all_ok &= r.ok;
                }
                Err(e) => {
                    messages.push(e);
                    all_ok = false;
                }
            }
        }
        Ok(plugin::PluginOpResult {
            ok: all_ok,
            message: messages.join("\n"),
            action: "plugin.op.update".to_string(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 卸载一个或多个插件（specs 为列表中的依赖 key，支持批量）。
/// 多个 key 时逐个执行 `dsh plugin remove`（全程只有一次 web 服务的启停封装），
/// 并逐个推送实时进度事件；结果逐条聚合返回。
#[tauri::command]
async fn plugin_remove(
    app: AppHandle,
    specs: Vec<String>,
    profile: String,
    channel: Channel<PipelineEvent>,
) -> Result<plugin::PluginOpResult, String> {
    let logger = app.state::<AppState>().logger.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let cleaned: Vec<String> = specs
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if cleaned.is_empty() {
            return Err(i18n::t("plugin.missing.remove.selection"));
        }
        let total = cleaned.len();
        let mut messages: Vec<String> = Vec::new();
        let mut all_ok = true;
        for (idx, spec) in cleaned.iter().enumerate() {
            // 实时进度：经 StepStarted 事件推给前端顶部展示（单个不带编号）。
            let title = if total == 1 {
                i18n::t_fmt("plugin.progress.removing.one", &[spec])
            } else {
                i18n::t_fmt(
                    "plugin.progress.removing",
                    &[&(idx + 1).to_string(), &total.to_string(), spec],
                )
            };
            let _ = channel.send(PipelineEvent::StepStarted {
                id: format!("item-{idx}"),
                title,
            });
            match plugin::run_plugin_op(
                &app,
                &profile,
                &["remove".to_string(), spec.clone()],
                "plugin.op.remove",
                spec,
                &channel,
                &logger,
            ) {
                Ok(r) => {
                    messages.push(r.message);
                    all_ok &= r.ok;
                }
                Err(e) => {
                    messages.push(e);
                    all_ok = false;
                }
            }
        }
        Ok(plugin::PluginOpResult {
            ok: all_ok,
            message: messages.join("\n"),
            action: "plugin.op.remove".to_string(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 修复安装：清理异常状态并分级重建 DeepSeek Harness 安装（详见 repair.rs 模块文档）。
#[tauri::command]
async fn repair_install(app: AppHandle, channel: Channel<PipelineEvent>) -> Result<(), String> {
    let logger = app.state::<AppState>().logger.clone();
    tauri::async_runtime::spawn_blocking(move || repair::repair(&app, &channel, &logger))
        .await
        .map_err(|e| e.to_string())?
}

// ─────────────────────────────── 启动 ───────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            default_parent_dir,
            detect_state,
            detect_tools,
            get_balance,
            install,
            check_for_updates,
            update,
            repair_install,
            uninstall_preview,
            cancel_uninstall_preview,
            uninstall,
            start_web,
            stop_web,
            open_terminal,
            open_web_ui,
            open_external,
            get_logs,
            get_log_dir,
            clear_logs,
            plugin_list,
            plugin_profiles,
            plugin_check_updates,
            plugin_install,
            plugin_update,
            plugin_remove,
        ])
        .setup(|app| {
            // 先按配置初始化界面语言（启动日志、错误提示等后端文案语言），再初始化日志器。
            let mut cfg = config::load_config(app.handle());
            i18n::set_lang(i18n::lang_from_config(&cfg.language));
            let logger = Logger::init(app.handle());
            // 启动只检测程序运行目录下的安装情况：无安装记录时自动采用
            // 同目录的 dsh / deepseek-harness 子目录（幂等，仅在发现时写入配置）。
            if detect::ensure_local_install(&mut cfg) {
                let _ = config::save_config(app.handle(), &cfg);
                logger.info(&crate::i18n::t_fmt(
                    "log.local_install_adopted",
                    &[cfg.install_dir.as_deref().unwrap_or("—")],
                ));
            }
            app.manage(AppState {
                web_pid: Mutex::new(None),
                logger: logger.clone(),
                preview_cancel: Mutex::new(None),
                plugin_updates: Mutex::new(None),
            });
            logger.info(&crate::i18n::t_fmt(
                "log.config_loaded",
                &[
                    cfg.install_dir.as_deref().unwrap_or("—"),
                    &cfg.auto_check_enabled.to_string(),
                    &cfg.language,
                ],
            ));
            spawn_auto_check(app.handle().clone());
            // 启动时探测全局 dsh 是否可识别 plugin 子命令；结果写入配置
            // （usePnpmDsh），不可识别时所有 dsh 命令改用 `pnpm dsh` 执行。
            let probe_handle = app.handle().clone();
            let probe_logger = logger.clone();
            std::thread::spawn(move || {
                let available = plugin::probe_global_dsh_plugin();
                let mut cfg = config::load_config(&probe_handle);
                cfg.use_pnpm_dsh = !available;
                let _ = config::save_config(&probe_handle, &cfg);
                if available {
                    probe_logger.info(&crate::i18n::t("log.dsh_probe_ok"));
                } else {
                    probe_logger.info(&crate::i18n::t("log.dsh_probe_fallback"));
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let RunEvent::Exit = event {
                if let Some(state) = app_handle.try_state::<AppState>() {
                    state.logger.info(&crate::i18n::t("log.exit"));
                }
            }
        });
}

/// 自动版本检测：启动 4 秒后立即检测一次，之后按配置间隔循环。
/// 除 DSH 自身更新外，同时按独立周期检测默认 profile 的插件更新并发出事件（供前端徽标提示）。
///
/// 两条独立线程：
/// - **DSH 更新**：间隔 `auto_check_interval_hours`，仅当 `auto_check_enabled` 时运行；
/// - **插件更新**：间隔 `plugin_auto_check_interval_hours`，仅当 `plugin_auto_check_enabled` 时运行。
fn spawn_auto_check(handle: AppHandle) {
    auto_check_dsh(handle.clone());
    auto_check_plugins(handle);
}

/// DSH 自身更新检测循环（沿用原有逻辑）。
fn auto_check_dsh(handle: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(4));
        loop {
            let cfg = config::load_config(&handle);
            if cfg.auto_check_enabled {
                if let Some(dir) = cfg.install_dir.clone() {
                    let mode = cfg.install_mode.clone();
                    let installed = if mode == "source" {
                        detect::is_valid_repo(std::path::Path::new(&dir))
                    } else {
                        detect::is_valid_prebuilt(std::path::Path::new(&dir))
                    };
                    if installed {
                        // 预构建模式以磁盘实时版本为准（配置可能残留旧值造成误报更新）。
                        let current_version = if mode == "source" {
                            cfg.installed_version.clone()
                        } else {
                            detect::read_pkg_version(std::path::Path::new(&dir))
                                .or_else(|| cfg.installed_version.clone())
                        };
                        if let Ok(result) = version::check_for_updates_in_dir(
                            std::path::Path::new(&dir),
                            &mode,
                            current_version.as_deref(),
                        ) {
                            let mut cfg2 = config::load_config(&handle);
                            cfg2.update_available = result.update_available;
                            cfg2.latest_commit = Some(result.remote_commit.clone());
                            cfg2.latest_subject = Some(result.subject.clone());
                            cfg2.last_check_at = Some(result.checked_at.clone());
                            let _ = config::save_config(&handle, &cfg2);
                            let _ = handle.emit("update-checked", &result);
                        }
                    }
                }
            }
            let hours = config::load_config(&handle).auto_check_interval_hours.max(1);
            std::thread::sleep(std::time::Duration::from_secs(hours * 3600));
        }
    });
}

/// 插件更新检查循环（默认 profile），独立于 DSH 检测。
/// 失败退坡：本轮检测结果含失败项（或整体查询失败）时，按 30 秒 → 5 分钟 → 20 分钟
/// 提前复查（封顶为配置间隔），不等到下个定时周期；全部正常则恢复配置间隔。
fn auto_check_plugins(handle: AppHandle) {
    std::thread::spawn(move || {
        // 比 DSH 检测稍晚，避免启动时并发争用。
        std::thread::sleep(std::time::Duration::from_secs(6));
        let mut fail_streak = 0u32;
        loop {
            let cfg = config::load_config(&handle);
            let mut all_resolved = true;
            if cfg.plugin_auto_check_enabled {
                if let Some(dir) = cfg.install_dir.clone() {
                    let mode = cfg.install_mode.clone();
                    let installed = if mode == "source" {
                        detect::is_valid_repo(std::path::Path::new(&dir))
                    } else {
                        detect::is_valid_prebuilt(std::path::Path::new(&dir))
                    };
                    if installed {
                        let profile = cfg.plugin_profile.trim().to_string();
                        if !profile.is_empty() {
                            match plugin::check_plugin_updates(&profile, &dir) {
                                Ok(updates) => {
                                    if updates.entries.iter().any(|e| e.error.is_some()) {
                                        all_resolved = false;
                                    }
                                    if let Some(state) = handle.try_state::<AppState>() {
                                        *state.plugin_updates.lock().unwrap() = Some(updates.clone());
                                    }
                                    let _ = handle.emit("plugin-updates-checked", &updates);
                                }
                                Err(_) => all_resolved = false,
                            }
                        }
                    }
                }
            }
            let hours = config::load_config(&handle).plugin_auto_check_interval_hours.max(1);
            let normal_secs = hours * 3600;
            let sleep_secs = if all_resolved {
                fail_streak = 0;
                normal_secs
            } else {
                fail_streak += 1;
                let backoff = match fail_streak {
                    1 => 30,
                    2 => 300,
                    _ => 1200,
                };
                normal_secs.min(backoff)
            };
            std::thread::sleep(std::time::Duration::from_secs(sleep_secs));
        }
    });
}
