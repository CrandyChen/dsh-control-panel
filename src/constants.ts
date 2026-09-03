// 前后端共享常量。

/** 控制面板自身版本（与 package.json / Cargo.toml / tauri.conf.json 保持同步）。 */
export const APP_VERSION = "2.4.4";

/** DeepSeek Harness Web UI 地址（与 Rust config::WEB_URL 同步）。 */
export const WEB_URL = "http://127.0.0.1:3080";

/** 源码安装时自动创建的子目录名（与 Rust config::repo_dir_name 默认值同步）。 */
export const REPO_DIR_NAME = "deepseek-harness";

/** 预构建内核（模式二）解压到的子目录名（与 Rust config::MODE2_DIR 同步）。 */
export const MODE2_DIR_NAME = "dsh";

/** 预构建内核发布仓库（含 release 资产 deepseek-harness-pkg-windows.zip）。 */
export const PREBUILT_PKG_REPO = "https://github.com/dsh-tauri-desk/deepseek-harness-pkg";

/** 插件推荐列表地址（GitHub 页面无法内嵌 iframe，需在系统浏览器打开）。 */
export const AWESOME_PLUGINS_URL = "https://github.com/AdamPlatin123/awesome-dsh-plugins";

/** TabBar 高度（px）。 */
export const TAB_BAR_HEIGHT = 44;

/**
 * 判定某个 URL 是否为 DSH 界面（用原生内嵌 Webview 承载，而非 iframe）。
 * 新版 DSH 内核的 web UI 使用 SameSite=Strict 会话 Cookie 与反点击劫持响应头，
 * 纯 iframe 无法完成认证与绕过限制，因此这类地址改为程序内原生 Webview 呈现。
 */
export function isDshUrl(url: string): boolean {
  return url.startsWith(WEB_URL);
}
