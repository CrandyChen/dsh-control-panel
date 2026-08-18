//! 运行环境工具检测：git / node / pnpm（必装项）+ python（推荐项）。
//!
//! 程序启动时自动执行，结果展示在「状态总览」；任一必装项缺失或版本过低时，
//! 安装流程会被拦截并引导用户在新 tab 查看安装指引。python 为推荐项，不阻塞安装。

use serde::Serialize;
use std::process::Command;

/// 单个工具的检测结果（序列化后字段为 camelCase）。
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    /// 工具标识：git / node / pnpm / python。
    pub id: String,
    /// 展示名称。
    pub name: String,
    /// 是否已安装（`--version` 能成功执行）。
    pub installed: bool,
    /// 检测到的版本（如 2.54.0 / 24.15.0 / 11.7.0 / 3.12.10）。
    pub version: Option<String>,
    /// 版本是否满足最低要求（必装项参与判定；git/python 无要求时恒为 installed）。
    pub ok: bool,
    /// 是否必装（缺失 / 版本过低会阻塞安装）。python 为推荐项，不阻塞。
    pub required: bool,
    /// 不满足最低要求时的人类可读说明。
    pub detail: Option<String>,
}

/// 依次尝试多个程序名执行 `--version`，返回第一条成功输出（stdout，空则回退 stderr）。
fn capture_version(programs: &[&str], args: &[&str]) -> Option<String> {
    for prog in programs {
        let out = match crate::process::no_window(Command::new(prog).args(args)).output() {
            Ok(o) => o,
            Err(_) => continue,
        };
        if !out.status.success() {
            continue;
        }
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !stdout.is_empty() {
            return Some(stdout);
        }
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if !stderr.is_empty() {
            return Some(stderr);
        }
    }
    None
}

/// 去掉版本字符串中的前导 'v' 与空白（node 输出如 `v24.15.0`）。
fn strip_v(s: &str) -> &str {
    s.trim().trim_start_matches('v')
}

/// 解析 major.minor（忽略 patch 与前导 v）。
fn major_minor(v: &str) -> Option<(u64, u64)> {
    let mut it = strip_v(v).split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    Some((major, minor))
}

/// Node 引擎要求：^22.19 || >=24。
fn node_version_ok(v: &str) -> bool {
    match major_minor(v) {
        Some((22, minor)) => minor >= 19,
        Some((major, _)) => major >= 24,
        None => false,
    }
}

/// pnpm 引擎要求：>=11.7。
fn pnpm_version_ok(v: &str) -> bool {
    match major_minor(v) {
        Some((major, minor)) => major > 11 || (major == 11 && minor >= 7),
        None => false,
    }
}

/// 从 `git --version` 输出提取版本（`git version 2.54.0.windows.1` → `2.54.0.windows.1`）。
fn parse_git(raw: &str) -> Option<String> {
    let t = raw.trim();
    t.strip_prefix("git version ")
        .map(str::to_string)
        .or_else(|| Some(t.to_string()))
}

/// 从 python 输出提取版本（`Python 3.12.10` → `3.12.10`）。
fn parse_python(raw: &str) -> Option<String> {
    let t = raw.trim();
    t.strip_prefix("Python ")
        .map(str::to_string)
        .or_else(|| Some(t.to_string()))
}

/// 组装单个工具的检测结果。
fn tool_status(
    id: &str,
    name: &str,
    required: bool,
    version: Option<String>,
    ok_check: fn(&str) -> bool,
    detail_if_bad: &str,
) -> ToolStatus {
    let installed = version.is_some();
    let ok = installed && version.as_deref().map(ok_check).unwrap_or(false);
    ToolStatus {
        id: id.into(),
        name: name.into(),
        installed,
        version,
        ok,
        required,
        detail: if installed && !ok {
            Some(detail_if_bad.into())
        } else {
            None
        },
    }
}

/// 检测全部工具。顺序：git / node / pnpm（必装）+ python（推荐）。
pub fn detect_tools() -> Vec<ToolStatus> {
    let git_raw = capture_version(&["git"], &["--version"]);
    let git_v = git_raw.as_deref().and_then(parse_git);

    let node_raw = capture_version(&["node"], &["--version"]);
    let node_v = node_raw.as_deref().map(|s| strip_v(s).to_string());

    let pnpm_raw = capture_version(&["pnpm.cmd", "pnpm"], &["--version"]);
    let pnpm_v = pnpm_raw.as_deref().map(|s| strip_v(s).to_string());

    let python_raw = capture_version(&["python", "py", "python3"], &["--version"]);
    let python_v = python_raw.as_deref().and_then(parse_python);

    vec![
        tool_status("git", "Git", true, git_v, |_| true, ""),
        tool_status(
            "node",
            "Node.js",
            true,
            node_v,
            node_version_ok,
            "需 Node.js ≥ 22.19 或 ≥ 24",
        ),
        tool_status("pnpm", "pnpm", true, pnpm_v, pnpm_version_ok, "需 pnpm ≥ 11.7"),
        tool_status("python", "Python", false, python_v, |_| true, ""),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_version_parsed() {
        assert_eq!(
            parse_git("git version 2.54.0.windows.1").as_deref(),
            Some("2.54.0.windows.1")
        );
        assert_eq!(parse_git("git version 2.47.0").as_deref(), Some("2.47.0"));
        assert_eq!(parse_git("  git version 2.0.0  ").as_deref(), Some("2.0.0"));
    }

    #[test]
    fn python_version_parsed() {
        assert_eq!(parse_python("Python 3.12.10").as_deref(), Some("3.12.10"));
        assert_eq!(parse_python("Python 3.12.10\n").as_deref(), Some("3.12.10"));
        assert_eq!(parse_python("3.12.10").as_deref(), Some("3.12.10"));
    }

    #[test]
    fn strip_leading_v() {
        assert_eq!(strip_v("v24.15.0"), "24.15.0");
        assert_eq!(strip_v("24.15.0"), "24.15.0");
        assert_eq!(strip_v("  v11.7.0  "), "11.7.0");
    }

    #[test]
    fn node_engine_requirement() {
        assert!(node_version_ok("v22.19.0"));
        assert!(node_version_ok("22.19.1"));
        assert!(node_version_ok("v24.15.0"));
        assert!(node_version_ok("25.0.0"));
        assert!(!node_version_ok("v22.18.0"));
        assert!(!node_version_ok("v20.11.1"));
        assert!(!node_version_ok("v23.0.0"));
        assert!(!node_version_ok("not-a-version"));
    }

    #[test]
    fn pnpm_engine_requirement() {
        assert!(pnpm_version_ok("11.7.0"));
        assert!(pnpm_version_ok("11.7.1"));
        assert!(pnpm_version_ok("12.0.0"));
        assert!(!pnpm_version_ok("11.6.0"));
        assert!(!pnpm_version_ok("10.0.0"));
        assert!(!pnpm_version_ok(""));
    }

    #[test]
    fn detect_tools_shape_and_order() {
        let tools = detect_tools();
        assert_eq!(tools.len(), 4);
        assert_eq!(tools[0].id, "git");
        assert_eq!(tools[1].id, "node");
        assert_eq!(tools[2].id, "pnpm");
        assert_eq!(tools[3].id, "python");
        // 必装项：git/node/pnpm；python 为推荐项。
        assert!(tools[0].required && tools[1].required && tools[2].required);
        assert!(!tools[3].required);
        // installed 时 ok 至少与 installed 一致（git/python 无版本要求）。
        for t in &tools {
            if t.id == "git" || t.id == "python" {
                assert_eq!(t.installed, t.ok);
            }
            assert!(t.version.is_none() || t.installed);
        }
    }

    #[test]
    fn version_too_low_sets_detail() {
        let st = tool_status("node", "Node.js", true, Some("v20.11.1".into()), node_version_ok, "需 Node.js ≥ 22.19 或 ≥ 24");
        assert!(st.installed);
        assert!(!st.ok);
        assert_eq!(st.detail.as_deref(), Some("需 Node.js ≥ 22.19 或 ≥ 24"));
    }
}
