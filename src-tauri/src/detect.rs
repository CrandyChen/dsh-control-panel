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

pub fn detect_state(dir: Option<&str>) -> DetectResult {
    let path = dir.map(Path::new);
    let valid = path.map(is_valid_repo).unwrap_or(false);
    DetectResult {
        installed: valid,
        valid,
        built: valid && path.map(is_built).unwrap_or(false),
        version: if valid { path.and_then(read_version) } else { None },
        running: is_running(),
        install_dir: dir.map(|s| s.to_string()),
        dsh_home: dsh_home(),
        // 实时读取 HEAD commit（config 中的 installed_commit 可能因手动 git pull 而陈旧）。
        installed_commit: if valid {
            path.and_then(crate::version::read_commit)
        } else {
            None
        },
    }
}

/// 手动安装的常见目录名（git 克隆；GitHub zip 解压无 .git，无法更新，不在检测范围）。
pub const MANUAL_INSTALL_DIR_NAMES: &[&str] = &[
    "deepseek-harness",
    "dsh",
    "deepseek-harness-main",
    "deepseek-harness-master",
];

/// 常见开发子目录名（用于盘符根与用户目录下的候选根）。
const DEV_SUBDIRS: &[&str] = &[
    "dev",
    "tools",
    "projects",
    "code",
    "source",
    "git",
    "repos",
    "workspace",
];

/// 候选根目录：所有存在的盘符根（含其下常见开发目录） + 用户目录及其常见子目录。
pub fn candidate_roots() -> Vec<std::path::PathBuf> {
    let mut roots = Vec::new();
    for d in b'a'..=b'z' {
        let root = std::path::PathBuf::from(format!("{}:\\", d as char));
        if !root.is_dir() {
            continue;
        }
        roots.push(root.clone());
        for sub in DEV_SUBDIRS {
            let p = root.join(sub);
            if p.is_dir() {
                roots.push(p);
            }
        }
        // 盘符根下 dev 的更深一层（如 D:\dev\tools\deepseek-harness）。
        for sub in DEV_SUBDIRS {
            let p = root.join("dev").join(sub);
            if p.is_dir() {
                roots.push(p);
            }
        }
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        let home = std::path::PathBuf::from(&home);
        roots.push(home.clone());
        for sub in DEV_SUBDIRS {
            let p = home.join(sub);
            if p.is_dir() {
                roots.push(p);
            }
        }
        let desktop = home.join("Desktop");
        if desktop.is_dir() {
            roots.push(desktop);
        }
    }
    roots
}

fn same_path(a: &Path, b: &Path) -> bool {
    let ca = a.canonicalize().unwrap_or_else(|_| a.to_path_buf());
    let cb = b.canonicalize().unwrap_or_else(|_| b.to_path_buf());
    ca == cb
}

/// 在给定根集合下扫描可能手动安装的 DeepSeek Harness（roots 可注入，便于测试）。
pub fn scan_with_roots(roots: &[std::path::PathBuf], exclude: Option<&str>) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for root in roots {
        for name in MANUAL_INSTALL_DIR_NAMES {
            let p = root.join(name);
            if !is_valid_repo(&p) {
                continue;
            }
            let s = p.to_string_lossy().to_string();
            if let Some(ex) = exclude {
                if !ex.trim().is_empty() && same_path(&p, Path::new(ex)) {
                    continue;
                }
            }
            if !found.contains(&s) {
                found.push(s);
            }
        }
    }
    found
}

/// 扫描本机可能手动安装的 DeepSeek Harness，返回有效仓库路径列表。
pub fn scan_manual_installs(exclude: Option<&str>) -> Vec<String> {
    scan_with_roots(&candidate_roots(), exclude)
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
    fn scan_finds_repo_in_roots_and_respects_exclude() {
        let base = std::env::temp_dir().join(format!("dsh-scan-{}", std::process::id()));
        let repo = base.join("deepseek-harness");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(
            repo.join("package.json"),
            r#"{"name":"@deepseek-ai/dsh-root","version":"0.1.0-rc.5"}"#,
        )
        .unwrap();
        // 非仓库目录不应命中
        std::fs::create_dir_all(base.join("dsh")).unwrap();

        let roots = vec![base.clone()];
        let found = scan_with_roots(&roots, None);
        assert_eq!(found.len(), 1);
        assert!(same_path(Path::new(&found[0]), &repo));

        let excluded = scan_with_roots(&roots, Some(repo.to_string_lossy().as_ref()));
        assert!(excluded.is_empty());

        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn scan_finds_nested_repo_dev_tools_pattern() {
        // 模拟 D:\dev\tools\deepseek-harness 结构。
        let base = std::env::temp_dir().join(format!("dsh-scan-nested-{}", std::process::id()));
        let repo = base.join("dev/tools/deepseek-harness");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(
            repo.join("package.json"),
            r#"{"name":"@deepseek-ai/dsh-root","version":"0.1.0-rc.5"}"#,
        )
        .unwrap();

        let roots = vec![base.join("dev/tools")];
        let found = scan_with_roots(&roots, None);
        assert_eq!(found.len(), 1);
        assert!(same_path(Path::new(&found[0]), &repo));

        std::fs::remove_dir_all(&base).unwrap();
    }
}
