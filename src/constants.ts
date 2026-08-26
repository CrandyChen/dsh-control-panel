// 前后端共享常量。

/** 控制面板自身版本（与 package.json / Cargo.toml / tauri.conf.json 保持同步）。 */
export const APP_VERSION = "2.1.1";

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

/** TabBar 高度（px），必须与 Rust src/tabs.rs 的 TAB_BAR_HEIGHT 保持一致。 */
export const TAB_BAR_HEIGHT = 44;
