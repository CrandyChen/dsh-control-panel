//! 运行环境工具检测：仅源码安装（source）需要 Git。
//!
//! 预构建内核（prebuilt）依赖内置 node/pnpm，无需任何外部运行环境，检测返回空。
//! 源码安装（source）用内置 node/pnpm + 外部 git；仅当 git 缺失或版本异常时，
//! 安装流程会被拦截并引导用户在新 tab 查看安装指引。

use serde::Serialize;
use std::process::Command;

/// 单个工具的检测结果（序列化后字段为 camelCase）。
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    /// 工具标识：仅 git。
    pub id: String,
    /// 展示名称。
    pub name: String,
    /// 是否已安装（`--version` 能成功执行）。
    pub installed: bool,
    /// 检测到的版本（如 2.54.0.windows.1）。
    pub version: Option<String>,
    /// 版本是否满足最低要求（git 无要求时恒为 installed）。
    pub ok: bool,
    /// 是否必装（git 为 true；缺失会阻塞源码安装）。
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

/// 从 `git --version` 输出提取版本（`git version 2.54.0.windows.1` → `2.54.0.windows.1`）。
fn parse_git(raw: &str) -> Option<String> {
    let t = raw.trim();
    t.strip_prefix("git version ")
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

/// 检测运行环境工具。按安装方式分派：
/// - `"prebuilt"`（预构建内核）：无需外部工具，返回空数组；
/// - `"source"`（源码安装）：仅检测外部 Git。
pub fn detect_tools(mode: &str) -> Vec<ToolStatus> {
    if mode != "source" {
        return Vec::new();
    }
    let git_raw = capture_version(&["git"], &["--version"]);
    let git_v = git_raw.as_deref().and_then(parse_git);
    vec![tool_status("git", "Git", true, git_v, |_| true, "")]
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
    fn prebuilt_mode_requires_no_tools() {
        assert!(detect_tools("prebuilt").is_empty());
    }

    #[test]
    fn source_mode_only_detects_git() {
        let tools = detect_tools("source");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].id, "git");
        assert!(tools[0].required);
        assert_eq!(tools[0].installed, tools[0].ok);
    }
}
