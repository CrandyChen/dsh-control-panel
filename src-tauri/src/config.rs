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

/// 预构建内核（模式二）全部 release 列表（供安装弹窗选择具体内核版本）。
pub const PREBUILT_PKG_RELEASES_API: &str =
    "https://api.github.com/repos/dsh-tauri-desk/deepseek-harness-pkg/releases";

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
    /// 已安装的内核版本注册表（多版本相互独立，共享 ~/.dsh 数据目录）。
    #[serde(default)]
    pub installed_kernels: Vec<KernelInstall>,
    /// 最近一次正常启动的内核 id（启动选择框的默认勾选项；首次为最新预构建内核）。
    #[serde(default)]
    pub last_started_kernel_id: Option<String>,
}

/// 一个已安装的 DSH 内核版本（预构建按版本独立目录，源码仅一份原地更新）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct KernelInstall {
    /// 唯一标识：prebuilt-<version>；源码模式固定为 "source"。
    pub id: String,
    /// 安装方式：source / prebuilt。
    pub mode: String,
    /// 版本号（预构建为归一化 tag，如 0.1.2-alpha.2；源码为 CLI 版本）。
    pub version: String,
    /// 安装目录（绝对路径；各版本相互独立）。
    pub install_dir: String,
    /// 源码模式取 commit；预构建为 None。
    pub commit: Option<String>,
    /// 安装时间。
    pub installed_at: String,
}

impl Default for KernelInstall {
    fn default() -> Self {
        Self {
            id: String::new(),
            mode: "prebuilt".to_string(),
            version: String::new(),
            install_dir: String::new(),
            commit: None,
            installed_at: String::new(),
        }
    }
}

// ─────────────────────────────── 内核注册表 ───────────────────────────────

/// 内核唯一标识：源码固定为 "source"（仅一份原地更新），预构建为 `prebuilt-<version>`。
pub fn kernel_id(mode: &str, version: &str) -> String {
    if mode == "source" {
        "source".to_string()
    } else {
        format!("prebuilt-{version}")
    }
}

/// 净化版本号以用作目录组件：非 `[A-Za-z0-9._-]` 字符替换为 `-`，去掉首尾点/横线并截断。
fn sanitize_dir_component(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let mut out = t.trim_matches(|c| c == '.' || c == '-').to_string();
    out.truncate(64);
    if out.is_empty() {
        out = "default".to_string();
    }
    out
}

/// 预构建内核某版本的安装目录：`<exe_dir>\dsh-<净化版本>`（与旧版单一 `dsh` 目录区分）。
pub fn prebuilt_version_dir(version: &str) -> PathBuf {
    exe_dir().join(format!("dsh-{}", sanitize_dir_component(version)))
}

/// 源码安装目录：`<exe_dir>\<repo 目录名>`（与旧版一致，仅一份）。
pub fn source_install_dir() -> PathBuf {
    exe_dir().join(repo_dir_name())
}

/// 依据安装方式与版本返回内核安装目录。
pub fn kernel_install_dir(mode: &str, version: &str) -> PathBuf {
    if mode == "source" {
        source_install_dir()
    } else {
        prebuilt_version_dir(version)
    }
}

/// 在注册表中按 id 查找内核。
pub fn find_kernel(cfg: &AppConfig, id: &str) -> Option<KernelInstall> {
    cfg.installed_kernels
        .iter()
        .find(|k| k.id == id)
        .cloned()
}

/// 注册表中是否已有同 (mode, version) 的内核（用于「已安装」标识与修复安装判别）。
#[allow(dead_code)]
pub fn is_kernel_installed(cfg: &AppConfig, mode: &str, version: &str) -> bool {
    if mode == "source" {
        cfg.installed_kernels.iter().any(|k| k.mode == "source")
    } else {
        cfg.installed_kernels
            .iter()
            .any(|k| k.mode == "prebuilt" && k.version == version)
    }
}

/// 最新（语义化版本最高）的预构建内核。
pub fn latest_prebuilt_kernel(cfg: &AppConfig) -> Option<KernelInstall> {
    let pre: Vec<&KernelInstall> = cfg
        .installed_kernels
        .iter()
        .filter(|k| k.mode == "prebuilt")
        .collect();
    pre.into_iter()
        .max_by(|a, b| compare_versions(&a.version, &b.version))
        .cloned()
}

/// 版本比较：优先语义化比较，解析失败退化为字典序。
fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    match (semver::Version::parse(a), semver::Version::parse(b)) {
        (Ok(va), Ok(vb)) => va.cmp(&vb),
        _ => a.cmp(b),
    }
}

/// 将指定内核设为「活动内核」：同步镜像字段（install_dir/mode/version/commit）并记录为最近启动。
pub fn set_active_kernel(cfg: &mut AppConfig, id: &str) {
    if let Some(k) = cfg.installed_kernels.iter().find(|k| k.id == id) {
        cfg.install_dir = Some(k.install_dir.clone());
        cfg.install_mode = k.mode.clone();
        cfg.installed_version = Some(k.version.clone());
        cfg.installed_commit = k.commit.clone();
        cfg.last_started_kernel_id = Some(k.id.clone());
    }
}

/// 插入或覆盖一个内核记录（按 id）。
pub fn upsert_kernel(cfg: &mut AppConfig, kernel: KernelInstall) {
    if let Some(existing) = cfg.installed_kernels.iter_mut().find(|k| k.id == kernel.id) {
        *existing = kernel;
    } else {
        cfg.installed_kernels.push(kernel);
    }
}

/// 从注册表移除内核；若其为最近启动项则清空该记录。
pub fn remove_kernel(cfg: &mut AppConfig, id: &str) {
    cfg.installed_kernels.retain(|k| k.id != id);
    if cfg.last_started_kernel_id.as_deref() == Some(id) {
        cfg.last_started_kernel_id = None;
    }
}

/// 解析当前活动内核：最近启动 id → 活动镜像（installDir 匹配） → 最新预构建 → 首条。
pub fn resolve_active_kernel(cfg: &AppConfig) -> Option<KernelInstall> {
    if let Some(id) = cfg.last_started_kernel_id.as_deref() {
        if let Some(k) = cfg.installed_kernels.iter().find(|k| k.id == id) {
            return Some(k.clone());
        }
    }
    if let Some(dir) = cfg.install_dir.as_deref() {
        if let Some(k) = cfg
            .installed_kernels
            .iter()
            .find(|k| k.install_dir == dir)
        {
            return Some(k.clone());
        }
    }
    latest_prebuilt_kernel(cfg).or_else(|| cfg.installed_kernels.first().cloned())
}

/// 迁移/补登：注册表为空但已安装过 → 以现有活动安装生成一条内核记录；已装但漏登 → 补登记。
/// 幂等，仅在发生变更时改造 cfg。
pub fn migrate_kernel_registry(cfg: &mut AppConfig) {
    if cfg.installed_kernels.is_empty() {
        if let Some(dir) = cfg.install_dir.as_ref() {
            if !dir.trim().is_empty() {
                let version = cfg
                    .installed_version
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string());
                let kernel = KernelInstall {
                    id: kernel_id(&cfg.install_mode, &version),
                    mode: cfg.install_mode.clone(),
                    version,
                    install_dir: dir.clone(),
                    commit: cfg.installed_commit.clone(),
                    installed_at: cfg
                        .last_updated_at
                        .clone()
                        .unwrap_or_else(now_string),
                };
                let kid = kernel.id.clone();
                cfg.installed_kernels.push(kernel);
                if cfg.last_started_kernel_id.is_none() {
                    cfg.last_started_kernel_id = Some(kid);
                }
            }
        }
        return;
    }
    // 已装但漏登（例如手动 git pull 后配置被重置）。
    if let Some(dir) = cfg.install_dir.as_ref() {
        let exists = cfg
            .installed_kernels
            .iter()
            .any(|k| k.install_dir == *dir);
        if !exists {
            let version = cfg
                .installed_version
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            let kernel = KernelInstall {
                id: kernel_id(&cfg.install_mode, &version),
                mode: cfg.install_mode.clone(),
                version,
                install_dir: dir.clone(),
                commit: cfg.installed_commit.clone(),
                installed_at: cfg
                    .last_updated_at
                    .clone()
                    .unwrap_or_else(now_string),
            };
            let kid = kernel.id.clone();
            cfg.installed_kernels.push(kernel);
            if cfg.last_started_kernel_id.is_none() {
                cfg.last_started_kernel_id = Some(kid);
            }
        }
    }
    // 活动镜像与注册表对齐：有内核但镜像为空（或指向已删内核）时，重算并同步。
    if !cfg.installed_kernels.is_empty() {
        if let Some(active) = resolve_active_kernel(cfg) {
            if cfg.install_dir.as_deref() != Some(active.install_dir.as_str()) {
                set_active_kernel(cfg, &active.id);
            }
        }
    }
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
            installed_kernels: Vec::new(),
            last_started_kernel_id: None,
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
    // 迁移/补登内核注册表（旧版单安装 → 内核记录；保持向后兼容）。
    migrate_kernel_registry(&mut cfg);
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

    fn sample_kernel(id: &str, mode: &str, version: &str, dir: &str) -> KernelInstall {
        KernelInstall {
            id: id.into(),
            mode: mode.into(),
            version: version.into(),
            install_dir: dir.into(),
            commit: None,
            installed_at: "2026-08-31 21:43:55".into(),
        }
    }

    #[test]
    fn kernel_id_is_version_scoped_for_prebuilt_and_stable_for_source() {
        assert_eq!(kernel_id("prebuilt", "0.1.2-alpha.2"), "prebuilt-0.1.2-alpha.2");
        assert_eq!(kernel_id("source", "0.1.1-rc.2"), "source");
    }

    #[test]
    fn prebuilt_version_dir_is_version_scoped() {
        let d = prebuilt_version_dir("0.1.2-alpha.2");
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(name, "dsh-0.1.2-alpha.2");
        // 非法字符被净化。
        let dd = prebuilt_version_dir("ab/c");
        let n2 = dd.file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(n2, "dsh-ab-c");
        // 源码目录固定为仓库名。
        let s = kernel_install_dir("source", "x");
        assert!(s.to_string_lossy().ends_with(repo_dir_name().as_str()));
    }

    #[test]
    fn upsert_find_and_remove_kernel_roundtrip() {
        let mut cfg = AppConfig::default();
        let k = sample_kernel("prebuilt-1", "prebuilt", "1.0.0", "D:\\dsh\\dsh-1.0.0");
        upsert_kernel(&mut cfg, k.clone());
        assert_eq!(cfg.installed_kernels.len(), 1);
        assert_eq!(find_kernel(&cfg, "prebuilt-1"), Some(k.clone()));

        // 同 id 覆盖。
        let k2 = KernelInstall { installed_at: "x".into(), ..k.clone() };
        upsert_kernel(&mut cfg, k2.clone());
        assert_eq!(cfg.installed_kernels.len(), 1);
        assert_eq!(cfg.installed_kernels[0].installed_at, "x");

        remove_kernel(&mut cfg, "prebuilt-1");
        assert!(cfg.installed_kernels.is_empty());
    }

    #[test]
    fn set_active_kernel_syncs_mirror_fields() {
        let mut cfg = AppConfig::default();
        cfg.installed_kernels.push(sample_kernel("prebuilt-a", "prebuilt", "2.0.0", "D:\\dsh\\dsh-2.0.0"));
        cfg.installed_kernels.push(sample_kernel("source", "source", "1.0.0", "D:\\deepseek-harness"));
        set_active_kernel(&mut cfg, "source");
        assert_eq!(cfg.install_mode, "source");
        assert_eq!(cfg.install_dir.as_deref(), Some("D:\\deepseek-harness"));
        assert_eq!(cfg.installed_version.as_deref(), Some("1.0.0"));
        assert_eq!(cfg.last_started_kernel_id.as_deref(), Some("source"));
    }

    #[test]
    fn is_kernel_installed_matches_mode_and_version() {
        let mut cfg = AppConfig::default();
        cfg.installed_kernels
            .push(sample_kernel("prebuilt-0.1.2-alpha.2", "prebuilt", "0.1.2-alpha.2", "D:\\dsh"));
        assert!(is_kernel_installed(&cfg, "prebuilt", "0.1.2-alpha.2"));
        assert!(!is_kernel_installed(&cfg, "prebuilt", "0.1.1"));
        // 源码：只要有一条 source 即视为已装。
        cfg.installed_kernels.push(sample_kernel("source", "source", "0.1.1-rc.2", "D:\\repo"));
        assert!(is_kernel_installed(&cfg, "source", "0.1.1-rc.2"));
    }

    #[test]
    fn latest_prebuilt_kernel_picks_highest_semver() {
        let mut cfg = AppConfig::default();
        cfg.installed_kernels
            .push(sample_kernel("prebuilt-0.1.0", "prebuilt", "0.1.0", "D:\\a"));
        cfg.installed_kernels
            .push(sample_kernel("prebuilt-0.1.2-alpha.2", "prebuilt", "0.1.2-alpha.2", "D:\\b"));
        cfg.installed_kernels
            .push(sample_kernel("prebuilt-0.1.11", "prebuilt", "0.1.11", "D:\\c"));
        let latest = latest_prebuilt_kernel(&cfg).unwrap();
        assert_eq!(latest.version, "0.1.11");
    }

    #[test]
    fn migrate_registry_from_single_install() {
        let mut cfg = AppConfig {
            install_dir: Some("D:\\dsh".into()),
            install_mode: "prebuilt".into(),
            installed_version: Some("0.1.2-alpha.2".into()),
            last_updated_at: Some("2026-08-31".into()),
            ..Default::default()
        };
        migrate_kernel_registry(&mut cfg);
        assert_eq!(cfg.installed_kernels.len(), 1);
        assert_eq!(cfg.installed_kernels[0].id, "prebuilt-0.1.2-alpha.2");
        assert_eq!(cfg.installed_kernels[0].install_dir, "D:\\dsh");
        assert_eq!(cfg.last_started_kernel_id.as_deref(), Some("prebuilt-0.1.2-alpha.2"));
    }
}
