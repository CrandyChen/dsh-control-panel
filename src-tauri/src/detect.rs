//! 安装 / 运行状态探测：是否安装、版本、是否已构建、是否在运行、DSH 数据目录。

use serde::Serialize;
use std::path::Path;

use crate::config::WEB_PORT;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DetectResult {
    /// 目录存在且是有效仓库。
    pub installed: bool,
    /// package.json name == @deepseek-ai/dsh-root 且含 .git。
    pub valid: bool,
    /// web 前端 dist 已构建（apps/web/dist）。
    pub built: bool,
    /// 当前安装版本。
    pub version: Option<String>,
    /// web 服务是否在运行（端口探测）。
    pub running: bool,
    /// 安装目录。
    pub install_dir: Option<String>,
    /// DSH 用户数据目录（$DSH_HOME，默认 ~/.dsh）。
    pub dsh_home: String,
    /// 当前 commit。
    pub installed_commit: Option<String>,
}

/// DSH 用户数据目录：优先 $DSH_HOME 环境变量，默认 ~/.dsh。
pub fn dsh_home() -> String {
    if let Ok(h) = std::env::var("DSH_HOME") {
        if !h.trim().is_empty() {
            return h;
        }
    }
    let home = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".to_string());
    format!("{home}\\.dsh")
}

/// 是否为有效的 DeepSeek Harness 仓库目录。
pub fn is_valid_repo(dir: &Path) -> bool {
    if !dir.join(".git").exists() {
        return false;
    }
    let pkg = dir.join("package.json");
    if !pkg.is_file() {
        return false;
    }
    match std::fs::read_to_string(&pkg) {
        Ok(s) => serde_json::from_str::<serde_json::Value>(&s)
            .ok()
            .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(|n| n.to_string()))
            == Some("@deepseek-ai/dsh-root".to_string()),
        Err(_) => false,
    }
}

/// 是否为有效的预构建内核安装目录（含 `node_modules\.bin\dsh.cmd`）。
pub fn is_valid_prebuilt(dir: &Path) -> bool {
    dir.join("node_modules").join(".bin").join("dsh.cmd").is_file()
}

/// 读取安装根的版本：预构建内核优先读 CLI 包版本（`node_modules/@deepseek-ai/dsh`，
/// 与 release tag 一致），其次根 `package.json`（打包外壳版本，可能与 release 脱节），
/// 最后 `apps/cli/package.json`。源码模式使用 `read_version`（apps/cli）。
pub fn read_pkg_version(dir: &Path) -> Option<String> {
    for rel in [
        "node_modules/@deepseek-ai/dsh/package.json",
        "package.json",
        "apps/cli/package.json",
    ] {
        let Ok(s) = std::fs::read_to_string(dir.join(rel)) else {
            continue;
        };
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
            if let Some(x) = v.get("version").and_then(|x| x.as_str()) {
                return Some(x.to_string());
            }
        }
    }
    None
}

/// 读 apps/cli/package.json 的 version 字段。
pub fn read_version(dir: &Path) -> Option<String> {
    let s = std::fs::read_to_string(dir.join("apps/cli/package.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    v.get("version").and_then(|x| x.as_str()).map(|s| s.to_string())
}

/// web 前端是否已构建（安装 / 更新流程的 pnpm run build 产物）。
pub fn is_built(dir: &Path) -> bool {
    dir.join("apps/web/dist").is_dir()
}

/// web 服务是否在运行（探测 127.0.0.1:3080）。
pub fn is_running() -> bool {
    crate::web::port_in_use(WEB_PORT)
}

pub fn detect_state(dir: Option<&str>, mode: &str) -> DetectResult {
    let path = dir.map(Path::new);
    let valid = if mode == "source" {
        path.map(is_valid_repo).unwrap_or(false)
    } else {
        path.map(is_valid_prebuilt).unwrap_or(false)
    };
    let built = if mode == "source" {
        valid && path.map(is_built).unwrap_or(false)
    } else {
        // 预构建内核已构建完成，无需重新构建。
        valid
    };
    let version = if valid {
        if mode == "source" {
            path.and_then(read_version)
        } else {
            path.and_then(read_pkg_version)
        }
    } else {
        None
    };
    DetectResult {
        installed: valid,
        valid,
        built,
        version,
        running: is_running(),
        install_dir: dir.map(|s| s.to_string()),
        dsh_home: dsh_home(),
        // 预构建内核无 git commit；源码模式实时读取 HEAD。
        installed_commit: if mode == "source" && valid {
            path.and_then(crate::version::read_commit)
        } else {
            None
        },
    }
}

/// 仅检测程序运行目录（exe 所在目录）下的 DSH 安装情况：
/// 预构建内核位于 `dsh` 子目录、源码安装位于 `deepseek-harness` 子目录。
/// 两者同时存在时优先预构建（默认安装方式）。返回 `(安装目录, 安装方式)`。
pub fn detect_local_default() -> Option<(String, &'static str)> {
    let prebuilt = crate::config::mode2_install_dir();
    if is_valid_prebuilt(&prebuilt) {
        return Some((prebuilt.to_string_lossy().to_string(), "prebuilt"));
    }
    let repo = crate::config::exe_dir().join(crate::config::repo_dir_name());
    if is_valid_repo(&repo) {
        return Some((repo.to_string_lossy().to_string(), "source"));
    }
    None
}

/// 无安装记录时自动采用程序运行目录下的既有安装，写入配置（幂等：仅在发生变更时保存）。
pub fn ensure_local_install(cfg: &mut crate::config::AppConfig) -> bool {
    if cfg
        .install_dir
        .as_deref()
        .map(|d| !d.trim().is_empty())
        .unwrap_or(false)
    {
        return false;
    }
    match detect_local_default() {
        Some((dir, mode)) => {
            cfg.install_dir = Some(dir);
            cfg.install_mode = mode.to_string();
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_repo_detected() {
        let tmp = std::env::temp_dir().join(format!("dsh-detect-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join(".git")).unwrap();
        std::fs::write(
            tmp.join("package.json"),
            r#"{"name":"something-else","version":"1.0.0"}"#,
        )
        .unwrap();
        assert!(!is_valid_repo(&tmp));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn valid_repo_requires_dsh_name() {
        let tmp = std::env::temp_dir().join(format!("dsh-detect2-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join(".git")).unwrap();
        std::fs::write(
            tmp.join("package.json"),
            r#"{"name":"@deepseek-ai/dsh-root","version":"0.1.0-rc.5"}"#,
        )
        .unwrap();
        assert!(is_valid_repo(&tmp));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn version_read_from_fixture() {
        let tmp = std::env::temp_dir().join(format!("dsh-ver-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("apps/cli")).unwrap();
        std::fs::write(
            tmp.join("apps/cli/package.json"),
            r#"{"name":"@deepseek-ai/dsh","version":"0.1.0-rc.5"}"#,
        )
        .unwrap();
        assert_eq!(read_version(&tmp).as_deref(), Some("0.1.0-rc.5"));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn pkg_version_prefers_cli_package_over_wrapper_root() {
        // 预构建内核：根 package.json 是打包外壳版本（0.1.1-rc.1），
        // CLI 包版本（0.1.1-rc.2）才是与 release tag 一致的版本。
        let tmp = std::env::temp_dir().join(format!("dsh-pkgver-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("node_modules/@deepseek-ai/dsh")).unwrap();
        std::fs::write(
            tmp.join("node_modules/@deepseek-ai/dsh/package.json"),
            r#"{"name":"@deepseek-ai/dsh","version":"0.1.1-rc.2"}"#,
        )
        .unwrap();
        std::fs::write(
            tmp.join("package.json"),
            r#"{"name":"deepseek-harness-pkg","version":"0.1.1-rc.1"}"#,
        )
        .unwrap();
        assert_eq!(read_pkg_version(&tmp).as_deref(), Some("0.1.1-rc.2"));
        // 无 CLI 包时回退根 package.json。
        std::fs::remove_dir_all(tmp.join("node_modules")).unwrap();
        assert_eq!(read_pkg_version(&tmp).as_deref(), Some("0.1.1-rc.1"));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn local_default_detects_prebuilt_and_source_dirs() {
        use crate::config::{exe_dir, mode2_install_dir, repo_dir_name};
        // 预构建内核目录：node_modules/.bin/dsh.cmd 存在即有效。
        let prebuilt = mode2_install_dir();
        let had_prebuilt = is_valid_prebuilt(&prebuilt);
        if !had_prebuilt {
            std::fs::create_dir_all(prebuilt.join("node_modules/.bin")).unwrap();
            std::fs::write(prebuilt.join("node_modules/.bin/dsh.cmd"), "@echo off").unwrap();
        }
        let found = detect_local_default().expect("应检出同目录预构建安装");
        assert_eq!(found.0, prebuilt.to_string_lossy().to_string());
        assert_eq!(found.1, "prebuilt");
        if !had_prebuilt {
            std::fs::remove_dir_all(&prebuilt).unwrap();
        }

        // 源码仓库目录：无预构建时才轮到它。
        let repo = exe_dir().join(repo_dir_name());
        let had_repo = is_valid_repo(&repo);
        if !had_repo && is_valid_prebuilt(&prebuilt) {
            // 环境里已有真实预构建目录时无法安全模拟（会先命中预构建），跳过该分支。
            return;
        }
        if !had_repo {
            std::fs::create_dir_all(repo.join(".git")).unwrap();
            std::fs::write(
                repo.join("package.json"),
                r#"{"name":"@deepseek-ai/dsh-root","version":"0.1.0-rc.5"}"#,
            )
            .unwrap();
        }
        let found = detect_local_default().expect("应检出同目录源码安装");
        assert_eq!(found.0, repo.to_string_lossy().to_string());
        assert_eq!(found.1, "source");
        if !had_repo {
            std::fs::remove_dir_all(&repo).unwrap();
        }
    }

    #[test]
    fn ensure_local_install_only_writes_when_missing() {
        // 已有安装记录时不覆盖。
        let mut cfg = crate::config::AppConfig::default();
        cfg.install_dir = Some("C:\\somewhere\\deepseek-harness".to_string());
        assert!(!ensure_local_install(&mut cfg));
        assert_eq!(cfg.install_dir.as_deref(), Some("C:\\somewhere\\deepseek-harness"));
    }
}
