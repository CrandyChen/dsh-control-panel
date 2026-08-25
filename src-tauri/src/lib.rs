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
mod i18n;
mod install;
mod logging;
mod net;
mod plugin;
mod prebuilt;
mod process;
mod repair;
mod terminal;
mod tools;
mod uninstall;
mod update;
mod version;
mod web;

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

#[tauri::command]
fn pick_directory(app: AppHandle) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let logger = app.state::<AppState>().logger.clone();
    let picked = app
        .dialog()
        .file()
        .blocking_pick_folder()
        .and_then(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().to_string());
    if let Some(p) = &picked {
        logger.info(&crate::i18n::t_fmt("log.picked_dir", &[p]));
    }
    picked
}

/// 源码安装模式的默认父目录（程序运行目录），供前端安装弹窗预填。
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

/// 检测运行环境工具（仅源码模式检测 Git；预构建模式返回空），启动时自动调用。
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

/// 扫描本机可能手动安装的 DeepSeek Harness。
#[tauri::command]
async fn scan_manual_installs(app: AppHandle) -> Vec<String> {
    tauri::async_runtime::spawn_blocking(move || {
        let logger = app.state::<AppState>().logger.clone();
        let cfg = config::load_config(&app);
        let found = detect::scan_manual_installs(cfg.install_dir.as_deref());
        logger.info(&crate::i18n::t_fmt("log.manual_scan_found", &[&found.len().to_string()]));
        for p in &found {
            logger.info(&crate::i18n::t_fmt("log.manual_scan_item", &[p]));
        }
        found
    })
    .await
    .unwrap_or_default()
}

/// 采用手动安装的 DeepSeek Harness：将安装目录/版本/commit 写入控制面板配置。
#[tauri::command]
async fn adopt_install(app: AppHandle, path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let logger = app.state::<AppState>().logger.clone();
        let p = PathBuf::from(&path);
        if !detect::is_valid_repo(&p) {
            return Err(error::AppError::NotValidInstall(path).friendly());
        }
        let version = detect::read_version(&p);
        let commit = version::read_commit(&p);
        let mut cfg = config::load_config(&app);
        cfg.install_dir = Some(path.clone());
        cfg.install_mode = "source".to_string();
        cfg.installed_version = version;
        cfg.installed_commit = commit;
        cfg.last_check_at = Some(config::now_string());
        config::save_config(&app, &cfg)?;
        logger.info(&crate::i18n::t_fmt(
            "log.adopt_done",
            &[
                &path,
                cfg.installed_version.as_deref().unwrap_or("—"),
                cfg.installed_commit.as_deref().unwrap_or("—"),
            ],
        ));
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn install(
    app: AppHandle,
    dir: String,
    mode: String,
    channel: Channel<PipelineEvent>,
) -> Result<(), String> {
    let logger = app.state::<AppState>().logger.clone();
    tauri::async_runtime::spawn_blocking(move || {
        install::install(&app, &dir, &mode, &channel, &logger)
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
    terminal::open_terminal(&dir)?;
    logger.info(&crate::i18n::t_fmt("log.open_terminal", &[&dir]));
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

/// 更新指定插件（spec 为列表中的依赖 key，github 标识需与安装时完全一致）。
#[tauri::command]
async fn plugin_update(
    app: AppHandle,
    spec: String,
    profile: String,
    channel: Channel<PipelineEvent>,
) -> Result<plugin::PluginOpResult, String> {
    let logger = app.state::<AppState>().logger.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if spec.trim().is_empty() {
            return Err(i18n::t("plugin.missing.update.spec"));
        }
        plugin::run_plugin_op(
            &app,
            &profile,
            &["update".to_string(), spec.clone()],
            "plugin.op.update",
            &spec,
            &channel,
            &logger,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 卸载一个或多个插件（specs 为列表中的依赖 key；「全部卸载」= 传全部条目）。
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
        let subject = cleaned.join("、");
        let mut args = vec!["remove".to_string()];
        args.extend(cleaned);
        plugin::run_plugin_op(
            &app,
            &profile,
            &args,
            "plugin.op.remove",
            &subject,
            &channel,
            &logger,
        )
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
            pick_directory,
            default_parent_dir,
            detect_state,
            detect_tools,
            scan_manual_installs,
            adopt_install,
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
            let cfg = config::load_config(app.handle());
            i18n::set_lang(i18n::lang_from_config(&cfg.language));
            let logger = Logger::init(app.handle());
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
/// 除 DSH 自身更新外，同时检测默认 profile 的插件更新并发出事件（供前端徽标提示）。
fn spawn_auto_check(handle: AppHandle) {
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
                        match version::check_for_updates_in_dir(
                            std::path::Path::new(&dir),
                            &mode,
                            current_version.as_deref(),
                        ) {
                            Ok(result) => {
                                let mut cfg2 = config::load_config(&handle);
                                cfg2.update_available = result.update_available;
                                cfg2.latest_commit = Some(result.remote_commit.clone());
                                cfg2.latest_subject = Some(result.subject.clone());
                                cfg2.last_check_at = Some(result.checked_at.clone());
                                let _ = config::save_config(&handle, &cfg2);
                                let _ = handle.emit("update-checked", &result);
                            }
                            Err(_) => { /* 后台检测失败静默，等待下个周期 */ }
                        }
                        // 插件更新检测（默认 profile）：失败静默，不阻断 DSH 检测。
                        let profile = cfg.plugin_profile.trim().to_string();
                        if !profile.is_empty() {
                            if let Ok(updates) = plugin::check_plugin_updates(&profile, &dir) {
                                if let Some(state) = handle.try_state::<AppState>() {
                                    *state.plugin_updates.lock().unwrap() = Some(updates.clone());
                                }
                                let _ = handle.emit("plugin-updates-checked", &updates);
                            }
                        }
                    }
                }
            }
            let hours = cfg.auto_check_interval_hours.max(1);
            std::thread::sleep(std::time::Duration::from_secs(hours * 3600));
        }
    });
}
