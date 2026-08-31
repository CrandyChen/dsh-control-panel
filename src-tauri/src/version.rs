//! 新版本检测：git fetch 后对比本地 HEAD 与 origin 默认分支。
//!
//! 默认分支动态探测（`git symbolic-ref refs/remotes/origin/HEAD`），
//! 失败时依次回退尝试 master / main。

use serde::Serialize;
use std::path::Path;

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

/// 读取当前 HEAD commit（短显示需求由前端截断）。
pub fn read_commit(dir: &Path) -> Option<String> {
    crate::gitops::rev_parse(dir, "HEAD").ok()
}

/// 探测默认远程分支（gitops：symbolic-ref，回退 master/main）。
pub fn default_branch(dir: &Path) -> Result<String, AppError> {
    crate::gitops::default_branch(dir)
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

/// 源码模式：git fetch → 对比本地 HEAD 与 origin 默认分支（基于 gitops/libgit2）。
fn check_for_updates_git(dir: &Path) -> Result<UpdateCheckResult, AppError> {
    crate::gitops::fetch(dir)?;
    let branch = default_branch(dir)?;
    let local_commit = crate::gitops::rev_parse(dir, "HEAD")?;
    let remote_ref = format!("origin/{branch}");
    let remote_commit = crate::gitops::rev_parse(dir, &remote_ref)?;
    let behind = crate::gitops::behind_count(dir, "HEAD", &remote_ref)?;
    let subject = crate::gitops::latest_subject(dir, &remote_ref)?;

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
/// 已安装的 CLI 版本比对。可剥离任意前置「渠道/来源」前缀（如 `dsh-`、`src-`、`v`）；
/// 无法识别时返回原 tag（保守，避免误判为无更新）。
pub fn normalized_tag_version(tag: &str) -> String {
    let s = tag.trim();
    let mut s = s.to_string();
    // 反复去掉已知前缀（dsh- / src- / v，逐个尝试，直到不再命中）。
    loop {
        let stripped = [
            "dsh-v-",
            "dsh-",
            "src-",
            "prebuilt-",
            "v",
        ]
        .iter()
        .find_map(|p| s.strip_prefix(p).map(|x| x.to_string()));
        match stripped {
            Some(next) if !next.is_empty() && next != s => s = next,
            _ => break,
        }
    }
    // 若剥离前缀后仍不以数字开头（未知前缀），从首个 `数字.数字.数字` 处截取版本起点。
    if !s.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
        if let Some(start) = find_semver_start(&s) {
            s = s[start..].to_string();
        }
    }
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

/// 在字符串中定位首个 `\d+.\d+.\d+` 版本起点的字节下标；无则返回 None。
fn find_semver_start(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let mut j = i;
            let read_num = |j: &mut usize| -> bool {
                let start = *j;
                while *j < bytes.len() && bytes[*j].is_ascii_digit() {
                    *j += 1;
                }
                *j > start
            };
            if read_num(&mut j) && j < bytes.len() && bytes[j] == b'.' {
                j += 1;
                if read_num(&mut j) && j < bytes.len() && bytes[j] == b'.' {
                    j += 1;
                    if read_num(&mut j) {
                        return Some(i);
                    }
                }
            }
            // 跳过连续数字，避免在数字之间重复尝试。
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    None
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

    #[test]
    fn normalized_tag_version_strips_src_and_other_prefixes() {
        // 发布方会给某些 release 打上 `src-` 前缀（作为来源标记），须剥掉再比较/排序。
        assert_eq!(
            normalized_tag_version("src-0.1.2-alpha.1"),
            "0.1.2-alpha.1"
        );
        assert_eq!(
            normalized_tag_version("src-0.1.2-alpha.2-12345"),
            "0.1.2-alpha.2"
        );
        assert_eq!(normalized_tag_version("prebuilt-1.2.3"), "1.2.3");
        // 未知前缀也能从首个数字版本处截取。
        assert_eq!(normalized_tag_version("misc-0.1.0-rc.1"), "0.1.0-rc.1");
        // 纯数字起始的版本不受影响。
        assert_eq!(normalized_tag_version("0.1.2-alpha.2"), "0.1.2-alpha.2");
    }
}
