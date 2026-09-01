//! 内嵌原生 Webview：在当前主窗口内嵌入一个用于显示 DSH 界面的子 Webview。
//!
//! 新版 DSH 内核的 web UI 使用 `SameSite=Strict` 会话 Cookie + 反点击劫持的响应头，
//! 纯前端 iframe（跨站）无法携带该 Cookie、也无法绕过点击劫持限制，会 401。
//! 这里改用 Tauri 2 的**原生子 Webview**，以「顶层导航」方式直接加载带 token 的访问
//! 地址，从而在程序内 Tab 打开 DSH 界面，而无需弹出系统浏览器。
//!
//! 实现约定与注意事项：
//! - 需要 tauri 的 `unstable` 特性（`Window::add_child` / `create_webview` 被该特性门控）。
//! - 主窗口只挂一个子 Webview（label = `dsh-embed`），复用于所有需要原生显示的 Tab，
//!   切换 Tab 时通过 `show` / `hide` + `navigate` 控制，避免多子 Webview 的资源问题。
//! - 坐标为**逻辑像素**、相对主窗口客户区；wry 按窗口缩放因子转物理像素并处理高分屏。
//! - 创建必须离开主线程（使用 `async` 命令 + `spawn_blocking`），否则 Windows 上会死锁
//!   （见 `WebviewBuilder::new` 文档中关于 Windows 同步命令死锁的说明）。
//! - 不将 token 写入任何日志：URL 仅经内存传递到 `navigate`。

use tauri::webview::WebviewBuilder;
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, Url, Webview, WebviewUrl};

/// 内嵌子 Webview 的固定 label（唯一，避免与主 Webview / 其它窗口冲突）。
pub const EMBED_LABEL: &str = "dsh-embed";
/// 承载内嵌 Webview 的主窗口 label（来自 tauri.conf.json 的默认窗口）。
pub const MAIN_WINDOW: &str = "main";

/// 内嵌 Webview 句柄的具体运行时类型（桌面端默认为 wry）。
pub type EmbedWebview = Webview<tauri::Wry>;

/// 读取当前已创建（若有）的内嵌 Webview 句柄。
fn current(app: &AppHandle) -> Option<EmbedWebview> {
    app.state::<crate::AppState>()
        .embed_webview
        .lock()
        .unwrap()
        .clone()
}

/// 确保内嵌 Webview 已创建并返回句柄；不存在则按给定逻辑尺寸挂载到主窗口。
/// 挂载后用 `about:blank` 起步，实际地址由 `navigate` 设置。
fn ensure_created(app: &AppHandle, x: f64, y: f64, w: f64, h: f64) -> Result<EmbedWebview, String> {
    if let Some(wv) = current(app) {
        return Ok(wv);
    }
    let window = app
        .get_window(MAIN_WINDOW)
        .ok_or_else(|| crate::i18n::t("embed.window_missing"))?;
    let builder = WebviewBuilder::new(
        EMBED_LABEL,
        WebviewUrl::External(Url::parse("about:blank").map_err(|e| e.to_string())?),
    );
    let created = window
        .add_child(
            builder,
            LogicalPosition::new(x, y),
            LogicalSize::new(w, h),
        )
        .map_err(|e| e.to_string())?;
    *app.state::<crate::AppState>().embed_webview.lock().unwrap() = Some(created.clone());
    Ok(created)
}

/// 幂等创建（或复用已存在的内嵌 Webview），导航到 `url`，并按给定逻辑尺寸定位后显示。
pub fn create(
    app: &AppHandle,
    url: &str,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> Result<(), String> {
    let wv = ensure_created(app, x, y, w, h)?;
    wv.navigate(Url::parse(url).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    wv.set_position(LogicalPosition::new(x, y)).map_err(|e| e.to_string())?;
    wv.set_size(LogicalSize::new(w, h)).map_err(|e| e.to_string())?;
    wv.show().map_err(|e| e.to_string())?;
    Ok(())
}

/// 仅导航（TAB 间 URL 变化时使用；Webview 已存在则复用）。
pub fn navigate(app: &AppHandle, url: &str) -> Result<(), String> {
    if let Some(wv) = current(app) {
        wv.navigate(Url::parse(url).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 重新对齐位置与尺寸（窗口缩放 / 高分屏变化时调用；坐标恒为逻辑像素）。
pub fn reposition(app: &AppHandle, x: f64, y: f64, w: f64, h: f64) -> Result<(), String> {
    if let Some(wv) = current(app) {
        wv.set_position(LogicalPosition::new(x, y)).map_err(|e| e.to_string())?;
        wv.set_size(LogicalSize::new(w, h)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 显示内嵌 Webview。
pub fn show(app: &AppHandle) -> Result<(), String> {
    if let Some(wv) = current(app) {
        wv.show().map_err(|e| e.to_string())?
    }
    Ok(())
}

/// 隐藏内嵌 Webview（切换到其它 Tab / 主界面 / 服务未就绪时调用）。
pub fn hide(app: &AppHandle) -> Result<(), String> {
    if let Some(wv) = current(app) {
        wv.hide().map_err(|e| e.to_string())?
    }
    Ok(())
}

/// 彻底关闭并移除内嵌 Webview（真正关闭对应 Tab 时可选调用）。
pub fn close(app: &AppHandle) -> Result<(), String> {
    if let Some(wv) = current(app) {
        wv.close().map_err(|e| e.to_string())?;
        *app.state::<crate::AppState>().embed_webview.lock().unwrap() = None;
    }
    Ok(())
}
