//! 控制面板自身配置的持久化（JSON，存放于应用数据目录，与 DSH 完全隔离）。
//!
//! 注意：这里保存的是「控制面板」的配置（安装目录、版本、检测结果等），
//! 绝不写入 DeepSeek Harness 安装目录或 ~/.dsh。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// DeepSeek Harness 官方仓库地址。
pub const REPO_URL: &str = "https://github.com/deepseek-ai/deepseek-harness.git";
/// DeepSeek Harness Web UI 默认地址。
pub const WEB_URL: &str = "http://127.0.0.1:3080";
pub const WEB_PORT: u16 = 3080;

/// 预构建内核（模式二）发布仓库：GitHub API 最新 release 与资产名。
pub const PREBUILT_PKG_API: &str =
    "https://api.github.com/repos/dsh-tauri-desk/deepseek-harness-pkg/releases/latest";
pub const PREBUILT_PKG_ASSET: &str = "deepseek-harness-pkg-windows.zip";

/// 预构建内核（模式二）解压到的子目录名（位于程序运行目录下）。
pub const MODE2_DIR: &str = "dsh";

/// 便携内置运行时子目录名（Node.js + pnpm，位于程序运行目录下）。
pub const RUNTIME_DIR: &str = "runtime";

/// 是否使用内置运行时（runtime 目录存在且有 node.exe）。
pub fn runtime_exists() -> bool {
    let dir = exe_dir().join(RUNTIME_DIR);
    dir.join("node.exe").is_file()
}

/// 便携内置运行时目录（程序运行目录下的 runtime）。
pub fn runtime_dir() -> PathBuf {
    exe_dir().join(RUNTIME_DIR)
}

/// 预构建内核（模式二）安装目录：程序运行目录下的 dsh 子目录。
pub fn mode2_install_dir() -> PathBuf {
    exe_dir().join(MODE2_DIR)
}

/// 源码安装（模式一）父目录：固定为程序运行目录（exe 所在目录）。
pub fn mode1_default_parent() -> PathBuf {
    exe_dir()
}

/// 子进程 PATH：优先把内置运行时目录前置（便携模式），否则返回系统 PATH。
pub fn augmented_path() -> String {
    let sys = std::env::var("PATH").unwrap_or_default();
    if runtime_exists() {
        let rt = runtime_dir();
        let rt_str = rt.to_string_lossy().to_string();
        if rt_str.is_empty() {
            return sys;
        }
        format!("{rt_str};{sys}")
    } else {
        sys
    }
}

/// 内置 pnpm 的可执行完整路径（便携运行时目录下的 pnpm），不存在时返回 None。
/// 用于保证源码安装等场景**优先使用自带 pnpm**（目标用户可能未全局安装 node/pnpm）。
pub fn bundled_pnpm_path() -> Option<String> {
    if !runtime_exists() {
        return None;
    }
    let rt = runtime_dir();
    for name in ["pnpm.exe", "pnpm.cmd"] {
        let p = rt.join(name);
        if p.is_file() {
            return Some(p.to_string_lossy().to_string());
        }
    }
    None
}

/// 实际使用的仓库地址：默认官方地址；可用环境变量 DSH_CONTROL_PANEL_REPO_URL 覆盖（镜像源 / 测试），
/// 兼容旧名 DSH_LAUNCHER_REPO_URL。
pub fn repo_url() -> String {
    std::env::var("DSH_CONTROL_PANEL_REPO_URL")
        .or_else(|_| std::env::var("DSH_LAUNCHER_REPO_URL"))
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| REPO_URL.to_string())
}

/// 从仓库 URL 提取 git clone 自动创建的末级目录名
/// （如 `…/deepseek-harness.git` → `deepseek-harness`）。
pub fn dir_name_from_url(url: &str) -> String {
    url.trim()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .map(|s| s.trim_end_matches(".git").to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "deepseek-harness".to_string())
}

/// 本次安装将自动创建的子目录名（git clone 目标末级目录）。
pub fn repo_dir_name() -> String {
    dir_name_from_url(&repo_url())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AppConfig {
    /// 安装目录（用户选择）。
    pub install_dir: Option<String>,
    /// 安装方式：source（从官方源码安装，需 Git）/ prebuilt（预构建内核，免 Git/pnpm）。
    pub install_mode: String,
    /// 当前安装版本（读自 apps/cli/package.json）。
    pub installed_version: Option<String>,
    /// 当前安装 commit。
    pub installed_commit: Option<String>,
    /// 上次更新时间。
    pub last_updated_at: Option<String>,
    /// 上次版本检测时间。
    pub last_check_at: Option<String>,
    /// 是否有新版本（git 远程提交对比）。
    pub update_available: bool,
    /// 远程最新 commit。
    pub latest_commit: Option<String>,
    /// 远程最新提交主题。
    pub latest_subject: Option<String>,
    /// 是否启用定时自动检测（DSH 自身更新）。
    pub auto_check_enabled: bool,
    /// 自动检测间隔（小时）。
    pub auto_check_interval_hours: u64,
    /// 是否启用定时自动检测插件更新（默认 profile）。
    pub plugin_auto_check_enabled: bool,
    /// 插件自动检测间隔（小时）。
    pub plugin_auto_check_interval_hours: u64,
    /// 是否使用 `pnpm dsh` 执行 dsh 命令（全局 `dsh` 不可识别时置 true；启动时自动探测）。
    pub use_pnpm_dsh: bool,
    /// 插件管理默认操作的 profile 名称。
    pub plugin_profile: String,
    /// 「打开界面」的默认打开方式：tab = 程序内新标签页；browser = 系统浏览器。
    pub open_ui_mode: String,
    /// 界面主题：auto（跟随系统）/ light / dark。选择后持久化，auto 随系统实时切换。
    pub theme: String,
    /// 界面语言：auto（跟随系统，非中英默认英文）/ zh-CN / en。
    pub language: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            install_dir: None,
            install_mode: "prebuilt".to_string(),
            installed_version: None,
            installed_commit: None,
            last_updated_at: None,
            last_check_at: None,
            update_available: false,
            latest_commit: None,
            latest_subject: None,
            auto_check_enabled: true,
            auto_check_interval_hours: 12,
            plugin_auto_check_enabled: true,
            plugin_auto_check_interval_hours: 12,
            use_pnpm_dsh: true,
            plugin_profile: "web".to_string(),
            open_ui_mode: "tab".to_string(),
            theme: "auto".to_string(),
            language: "auto".to_string(),
        }
    }
}

/// 程序（exe）所在目录：配置与日志随程序走（portable 语义）。
pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(std::env::temp_dir)
}

/// 旧版本配置位置（旧 identifier 为 com.dsh.launcher，位于 %LOCALAPPDATA%），
/// 用于便携化改造前用户的首次迁移。新版本 identifier 为 com.dsh.controlpanel。
fn legacy_config_path(_app: &tauri::AppHandle) -> Option<PathBuf> {
    let local = std::env::var("LOCALAPPDATA").ok()?;
    Some(PathBuf::from(local).join("com.dsh.launcher").join("config.json"))
}

pub fn config_path(_app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(exe_dir().join("config.json"))
}

/// 依据安装目录的实际形态推断安装方式（源码头 → source；预构建 dsh.cmd → prebuilt）。
/// 无安装目录 / 无法识别时回退默认 prebuilt。幂等：与磁盘现状保持一致。
fn infer_install_mode(install_dir: Option<&str>) -> String {
    match install_dir {
        Some(dir) => {
            let p = PathBuf::from(dir);
            if crate::detect::is_valid_repo(&p) {
                "source".to_string()
            } else if crate::detect::is_valid_prebuilt(&p) {
                "prebuilt".to_string()
            } else {
                "prebuilt".to_string()
            }
        }
        None => "prebuilt".to_string(),
    }
}

pub fn load_config(app: &tauri::AppHandle) -> AppConfig {
    let path = match config_path(app) {
        Ok(p) => p,
        Err(_) => return AppConfig::default(),
    };
    let cfg = match fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => {
            // 新位置无配置：若旧位置存在则迁移（保留已采用的安装目录等参数）。
            if let Some(legacy) = legacy_config_path(app) {
                if legacy.is_file() {
                    if let Ok(s) = fs::read_to_string(&legacy) {
                        if let Ok(cfg) = serde_json::from_str::<AppConfig>(&s) {
                            let _ = save_config(app, &cfg);
                            return cfg;
                        }
                    }
                }
            }
            AppConfig::default()
        }
    };
    // 修正 install_mode：与磁盘现状保持一致（可能因手动处理或旧配置而不准）。
    let mut cfg = cfg;
    cfg.install_mode = infer_install_mode(cfg.install_dir.as_deref());
    cfg
}

pub fn save_config(app: &tauri::AppHandle, cfg: &AppConfig) -> Result<(), String> {
    let path = config_path(app)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    fs::write(&path, json).map_err(|e| e.to_string())
}

pub fn now_string() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

pub fn date_string() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_roundtrip() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.auto_check_enabled, true);
        assert_eq!(cfg.auto_check_interval_hours, 12);
        assert_eq!(cfg.plugin_auto_check_enabled, true);
        assert_eq!(cfg.plugin_auto_check_interval_hours, 12);
        assert_eq!(cfg.install_dir, None);
        assert_eq!(cfg.use_pnpm_dsh, true);
        assert_eq!(cfg.plugin_profile, "web");
        assert_eq!(cfg.open_ui_mode, "tab");
        let json = serde_json::to_string(&cfg).unwrap();
        let back: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn config_serde_uses_camel_case() {
        let cfg = AppConfig {
            install_dir: Some("D:/x".into()),
            use_pnpm_dsh: false,
            open_ui_mode: "browser".into(),
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"installDir\""));
        assert!(!json.contains("\"install_dir\""));
        assert!(json.contains("\"usePnpmDsh\""));
        assert!(json.contains("\"pluginProfile\""));
        assert!(json.contains("\"openUiMode\""));
        assert!(!json.contains("\"open_ui_mode\""));
    }

    #[test]
    fn config_missing_open_ui_mode_defaults_to_tab() {
        // 旧配置无 openUiMode 字段：serde default 补全为 "tab"。
        let json = r#"{"installDir":"D:/x"}"#;
        let cfg: AppConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.open_ui_mode, "tab");
    }

    #[test]
    fn config_missing_plugin_auto_check_fields_default_on() {
        // 旧配置无 plugin 自动检测字段：serde default 补全为开启 + 12 小时。
        let json = r#"{"installDir":"D:/x"}"#;
        let cfg: AppConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.plugin_auto_check_enabled, true);
        assert_eq!(cfg.plugin_auto_check_interval_hours, 12);
    }

    #[test]
    fn dir_name_from_repo_url() {
        assert_eq!(
            dir_name_from_url("https://github.com/deepseek-ai/deepseek-harness.git"),
            "deepseek-harness"
        );
        assert_eq!(
            dir_name_from_url("https://github.com/deepseek-ai/deepseek-harness"),
            "deepseek-harness"
        );
        assert_eq!(
            dir_name_from_url("https://mirror.example.com/x/repo.git/"),
            "repo"
        );
        assert_eq!(dir_name_from_url(""), "deepseek-harness");
        assert_eq!(dir_name_from_url("https://github.com/a/b.git"), "b");
    }
}
