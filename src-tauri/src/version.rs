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

/// 执行完整检测：fetch → 对比 → 结果。
pub fn check_for_updates_in_dir(dir: &Path) -> Result<UpdateCheckResult, AppError> {
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
}
