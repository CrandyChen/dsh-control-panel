//! 新版本检测：git fetch 后对比本地 HEAD 与 origin 默认分支。
//!
//! 默认分支动态探测（`git symbolic-ref refs/remotes/origin/HEAD`），
//! 失败时依次回退尝试 master / main。

use serde::Serialize;
use std::path::Path;
use std::process::Command;

use crate::error::AppError;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckResult {
    pub update_available: bool,
    pub local_commit: String,
    pub remote_commit: String,
    pub behind: u64,
    pub subject: String,
    pub checked_at: String,
}

/// 在安装目录执行 git 并捕获 stdout（禁止密码交互，避免挂起；隐藏控制台窗口）。
pub fn run_git_capture(dir: &Path, args: &[String]) -> Result<String, AppError> {
    let out = crate::process::no_window(
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_TERMINAL_PROMPT", "0"),
    )
    .output()
    .map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AppError::ProgramNotFound("git".into())
        } else {
            AppError::Network(e.to_string())
        }
    })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(AppError::Network(if stderr.is_empty() {
            format!("git {} 退出码 {:?}", args.join(" "), out.status.code())
        } else {
            stderr
        }));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git_args(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

/// 从 `git symbolic-ref refs/remotes/origin/HEAD` 的输出解析分支名。
pub fn extract_branch_from_symbolic_ref(output: &str) -> Option<String> {
    let t = output.trim().trim_start_matches("refs/remotes/origin/");
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// 解析 `git rev-list --count` 的输出。
pub fn parse_behind_count(output: &str) -> u64 {
    output.trim().parse().unwrap_or(0)
}

/// 读取当前 HEAD commit（短显示需求由前端截断）。
pub fn read_commit(dir: &Path) -> Option<String> {
    run_git_capture(dir, &git_args(&["rev-parse", "HEAD"])).ok()
}

/// 探测默认远程分支。
pub fn default_branch(dir: &Path) -> Result<String, AppError> {
    if let Ok(out) = run_git_capture(
        dir,
        &git_args(&["symbolic-ref", "refs/remotes/origin/HEAD"]),
    ) {
        if let Some(b) = extract_branch_from_symbolic_ref(&out) {
            return Ok(b);
        }
    }
    for candidate in ["master", "main"] {
        if run_git_capture(
            dir,
            &git_args(&["rev-parse", "--verify", &format!("origin/{candidate}")]),
        )
        .is_ok()
        {
            return Ok(candidate.to_string());
        }
    }
    Err(AppError::Network(
        "无法确定远程默认分支（origin/HEAD 与 master/main 均不可用）".into(),
    ))
}

/// 执行完整检测。`mode` = "prebuilt" 时走 GitHub release 比对（无需 git）；
/// `mode` = "source" 时走 git fetch → 对比。`current_version` 用于预构建模式
/// 比对当前安装的 tag（源码模式忽略）。
pub fn check_for_updates_in_dir(
    dir: &Path,
    mode: &str,
    current_version: Option<&str>,
) -> Result<UpdateCheckResult, AppError> {
    if mode == "source" {
        check_for_updates_git(dir)
    } else {
        check_for_updates_prebuilt(current_version)
    }
}

/// 源码模式：git fetch → 对比本地 HEAD 与 origin 默认分支。
fn check_for_updates_git(dir: &Path) -> Result<UpdateCheckResult, AppError> {
    run_git_capture(dir, &git_args(&["fetch", "origin"]))?;
    let branch = default_branch(dir)?;

    let local_commit = run_git_capture(dir, &git_args(&["rev-parse", "HEAD"]))?;
    let remote_ref = format!("origin/{branch}");
    let remote_commit = run_git_capture(dir, &git_args(&["rev-parse", &remote_ref]))?;
    let behind = parse_behind_count(&run_git_capture(
        dir,
        &git_args(&["rev-list", "--count", &format!("HEAD..{remote_ref}")]),
    )?);
    let subject = run_git_capture(dir, &git_args(&["log", "-1", "--format=%s", &remote_ref]))?;

    Ok(UpdateCheckResult {
        update_available: behind > 0,
        local_commit,
        remote_commit,
        behind,
        subject,
        checked_at: crate::config::now_string(),
    })
}

/// 从 release tag 提取「版本号」：tag 形如 `dsh-0.1.1-rc.2-32485170079`
/// （发布方在 semver 后追加了 `-<构建/提交号>`）。提取出 `0.1.1-rc.2` 用于与
/// 已安装的 CLI 版本比对；无法识别时返回原 tag（保守，避免误判为无更新）。
pub fn normalized_tag_version(tag: &str) -> String {
    let s = tag.trim();
    // 去掉常见前缀 dsh- / v。
    let s = s
        .strip_prefix("dsh-")
        .or_else(|| s.strip_prefix("v"))
        .unwrap_or(s);
    let mut s = s.to_string();
    // 反复去掉末尾的 `-<纯数字>`（构建/提交号），如 `-32485170079`。
    loop {
        let Some(idx) = s.rfind('-') else { break };
        let (prefix, rest) = s.split_at(idx);
        let suffix = &rest[1..];
        if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
            s = prefix.to_string();
        } else {
            break;
        }
    }
    s
}

/// 预构建模式：查询 GitHub 最新 release tag，与当前安装 tag 比对。
/// 比较时以归一化后的版本号为准（tag 可能带 `dsh-` 前缀与 `-<提交号>` 后缀，
/// 与已安装的 CLI 版本不完全一致）。
fn check_for_updates_prebuilt(current_version: Option<&str>) -> Result<UpdateCheckResult, AppError> {
    let release = crate::prebuilt::latest_release()?;
    let current = current_version.unwrap_or("").trim();
    let latest_v = normalized_tag_version(&release.tag);
    let update_available = !latest_v.is_empty() && latest_v != current;
    Ok(UpdateCheckResult {
        update_available,
        local_commit: current.to_string(),
        remote_commit: release.tag.clone(),
        behind: if update_available { 1 } else { 0 },
        subject: format!("{}（{}）", release.tag, release.url),
        checked_at: crate::config::now_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_extraction() {
        assert_eq!(
            extract_branch_from_symbolic_ref("refs/remotes/origin/master\n"),
            Some("master".into())
        );
        assert_eq!(
            extract_branch_from_symbolic_ref("refs/remotes/origin/main"),
            Some("main".into())
        );
        assert_eq!(extract_branch_from_symbolic_ref("  \n"), None);
    }

    #[test]
    fn behind_count_parsing() {
        assert_eq!(parse_behind_count("42\n"), 42);
        assert_eq!(parse_behind_count("0"), 0);
        assert_eq!(parse_behind_count("abc"), 0);
    }

    #[test]
    fn normalized_tag_version_strips_prefix_and_build_suffix() {
        // 发布方 tag：dsh-<semver>-<默认数字提交号>。
        assert_eq!(
            normalized_tag_version("dsh-0.1.1-rc.2-32485170079"),
            "0.1.1-rc.2"
        );
        // 纯 semver 无后缀。
        assert_eq!(normalized_tag_version("0.1.1-rc.2"), "0.1.1-rc.2");
        // v 前缀。
        assert_eq!(normalized_tag_version("v2.0.0"), "2.0.0");
        // 数字后缀。
        assert_eq!(normalized_tag_version("0.1.1-rc.2-123"), "0.1.1-rc.2");
        // 非纯数字后缀（如含字母的 git 短 SHA）：保守保留归一化后的前缀。
        assert_eq!(
            normalized_tag_version("dsh-0.1.1-rc.2-3248517a"),
            "0.1.1-rc.2-3248517a"
        );
    }
}
