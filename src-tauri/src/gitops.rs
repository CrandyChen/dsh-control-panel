//! 基于 git2 (libgit2) 的 git 操作封装。
//!
//! 目标：源码安装 / 更新 / 修复不再依赖外部 Git 环境，全部 git 操作在本进程内完成。
//! clone/fetch/rev-parse/behind-count/log-subject/symbolic-ref/reset/ls-remote 均由 libgit2
//! 完成；`git clean`（清理未跟踪/忽略文件）libgit2 未提供，以 worktree 遍历 + 状态标记
//! 实现（等效 `git clean -fdx`，并保留 node_modules / .venv / .git，与旧命令一致）。
//!
//! 网络错误在调用前一般已由 `net::ensure_repo_reachable` 预检，此处错误多为 git 层级问题。

use std::path::{Path, PathBuf};

use git2::build::RepoBuilder;
use git2::{Direction, FetchOptions, RemoteCallbacks, Repository, ResetType, StatusOptions};
use tauri::ipc::Channel;

use crate::error::AppError;
use crate::process::PipelineEvent;

/// GitHub 网络操作（clone / fetch / ls-remote）的最大尝试次数（国内常抽筋，多试几次）。
const GIT_NET_RETRY: u32 = 3;
/// 每次失败后的等待（毫秒），随尝试次数递增。
const GIT_NET_RETRY_DELAY_BASE_MS: u64 = 1200;

/// 将 git2 错误统一映射为 `AppError::Git`。
fn gerr(e: git2::Error) -> AppError {
    AppError::Git(e.to_string())
}

/// 打开安装目录下的仓库。
fn open_repo(dir: &Path) -> Result<Repository, AppError> {
    Repository::open(dir).map_err(|_| {
        AppError::Git(format!(
            "不是有效的 Git 仓库：{}",
            dir.to_string_lossy()
        ))
    })
}

/// 克隆仓库到目标目录，实时推送传输进度（step 用于进度归属）。
/// GitHub 在国内常抽筋：失败自动重试多次（每次递增等待）。
pub fn clone_with_progress(
    url: &str,
    target: &Path,
    channel: &Channel<PipelineEvent>,
    step: &str,
) -> Result<(), AppError> {
    let mut last_err: Option<AppError> = None;
    for attempt in 0..GIT_NET_RETRY {
        // 失败残留的目录会阻止 git2 再次克隆：重试前清空。
        if target.exists() {
            let _ = std::fs::remove_dir_all(target);
        }
        match clone_once(url, target, channel, step) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(
                    GIT_NET_RETRY_DELAY_BASE_MS * (attempt as u64 + 1),
                ));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| AppError::Git("克隆仓库失败".into())))
}

/// 单次克隆（无重试）。
fn clone_once(
    url: &str,
    target: &Path,
    channel: &Channel<PipelineEvent>,
    step: &str,
) -> Result<(), AppError> {
    let mut cb = RemoteCallbacks::new();
    let ch = channel.clone();
    let step_s = step.to_string();
    cb.transfer_progress(move |p| {
        let _ = ch.send(PipelineEvent::DownloadProgress {
            step: step_s.clone(),
            received: p.received_objects() as u64,
            total: p.total_objects() as u64,
            speed_bps: 0,
        });
        true
    });
    let mut fo = FetchOptions::new();
    fo.remote_callbacks(cb);
    let mut builder = RepoBuilder::new();
    builder.fetch_options(fo);
    builder.clone(url, target).map_err(gerr)?;
    Ok(())
}

/// `git fetch origin`：拉取远程引用（快进到 `refs/remotes/origin/*`）。
/// GitHub 在国内常抽筋：失败自动重试多次。
pub fn fetch(dir: &Path) -> Result<(), AppError> {
    let mut last_err: Option<AppError> = None;
    for attempt in 0..GIT_NET_RETRY {
        match fetch_once(dir) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(
                    GIT_NET_RETRY_DELAY_BASE_MS * (attempt as u64 + 1),
                ));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| AppError::Git("fetch origin 失败".into())))
}

/// 单次 fetch（无重试）。
fn fetch_once(dir: &Path) -> Result<(), AppError> {
    let repo = open_repo(dir)?;
    let mut remote = repo.find_remote("origin").map_err(gerr)?;
    let mut fo = FetchOptions::new();
    remote
        .fetch(&["+refs/heads/*:refs/remotes/origin/*"], Some(&mut fo), None)
        .map_err(gerr)?;
    Ok(())
}

/// 读取某个引用（或表达式，如 HEAD / origin/main）解析出的 commit OID。
pub fn rev_parse(dir: &Path, rev: &str) -> Result<String, AppError> {
    let repo = open_repo(dir)?;
    let obj = repo.revparse_single(rev).map_err(gerr)?;
    Ok(obj.id().to_string())
}

/// 探测默认远程分支：优先 `refs/remotes/origin/HEAD`，回退 master / main。
pub fn default_branch(dir: &Path) -> Result<String, AppError> {
    let repo = open_repo(dir)?;
    if let Ok(r) = repo.find_reference("refs/remotes/origin/HEAD") {
        if let Some(target) = r.symbolic_target() {
            if let Some(name) = target.rsplit('/').next() {
                if !name.is_empty() {
                    return Ok(name.to_string());
                }
            }
        }
    }
    for cand in ["master", "main"] {
        if repo
            .find_branch(&format!("origin/{cand}"), git2::BranchType::Remote)
            .is_ok()
        {
            return Ok(cand.to_string());
        }
    }
    Err(AppError::Git("无法确定远程默认分支（origin/HEAD 与 master/main 均不可用）".into()))
}

/// 统计 `remote_ref` 相对 `base_ref` 落后的提交数（`git rev-list --count base..remote`）。
pub fn behind_count(dir: &Path, base: &str, remote: &str) -> Result<u64, AppError> {
    let repo = open_repo(dir)?;
    let base_oid = repo.revparse_single(base).map_err(gerr)?.id();
    let remote_oid = repo.revparse_single(remote).map_err(gerr)?.id();
    let mut walk = repo.revwalk().map_err(gerr)?;
    walk.push(remote_oid).map_err(gerr)?;
    walk.hide(base_oid).map_err(gerr)?;
    Ok(walk.count() as u64)
}

/// 读取 `remote_ref` 指向提交的标题（`git log -1 --format=%s`）。
pub fn latest_subject(dir: &Path, remote_ref: &str) -> Result<String, AppError> {
    let repo = open_repo(dir)?;
    let oid = repo.revparse_single(remote_ref).map_err(gerr)?.id();
    let commit = repo.find_commit(oid).map_err(gerr)?;
    Ok(commit.summary().unwrap_or("").to_string())
}

/// `git reset --hard <target>`：将工作区与索引硬重置到目标。
pub fn reset_hard(dir: &Path, target: &str) -> Result<(), AppError> {
    let repo = open_repo(dir)?;
    let obj = repo.revparse_single(target).map_err(gerr)?;
    repo.reset(&obj, ResetType::Hard, None).map_err(gerr)?;
    Ok(())
}

/// 判断某条（相对于 workdir、使用正斜杠的）路径是否应被 `clean` 保留。
/// 与旧命令 `git clean -fdx -e node_modules -e .venv` 行为一致：保留这三类目录。
fn clean_protected(rel: &str) -> bool {
    let first = rel.split('/').next().unwrap_or("");
    first == "node_modules" || first == ".venv" || first == ".git"
}

/// `git clean -fdx`：删除未被 Git 跟踪且非受保护（node_modules/.venv/.git）的文件/目录。
pub fn clean(dir: &Path) -> Result<(), AppError> {
    let repo = open_repo(dir)?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| AppError::Git("仓库无 worktree".into()))?
        .to_path_buf();
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .include_ignored(true)
        .recurse_untracked_dirs(false);
    let statuses = repo.statuses(Some(&mut opts)).map_err(gerr)?;
    for entry in statuses.iter() {
        let st = entry.status();
        let is_untracked_ignored = st.intersects(git2::Status::WT_NEW | git2::Status::IGNORED);
        if !is_untracked_ignored {
            continue;
        }
        let Some(rel) = entry.path() else { continue };
        if clean_protected(rel) {
            continue;
        }
        let full = workdir.join(rel);
        if full.is_dir() {
            let _ = std::fs::remove_dir_all(&full);
        } else {
            let _ = std::fs::remove_file(&full);
        }
    }
    Ok(())
}

/// 生成一个唯一的临时目录（用于 ls-remote 的匿名仓库句柄）。
fn temp_repo_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("dsh-gitops-ls-{}-{}", std::process::id(), nanos))
}

/// `git ls-remote --tags <url>`：列出远程仓库的 tag 名（不含 `refs/tags/` 前缀）。
/// libgit2 无开箱的 ls-remote，用匿名 remote 连接后 `Remote::list()` 读取。
/// GitHub 常抽筋：失败自动重试多次。
pub fn ls_remote_tags(url: &str) -> Result<Vec<String>, AppError> {
    let mut last_err: Option<AppError> = None;
    for attempt in 0..GIT_NET_RETRY {
        match ls_remote_tags_once(url) {
            Ok(t) => return Ok(t),
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(
                    GIT_NET_RETRY_DELAY_BASE_MS * (attempt as u64 + 1),
                ));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| AppError::Git("ls-remote tags 失败".into())))
}

/// 单次 ls-remote（无重试）。
fn ls_remote_tags_once(url: &str) -> Result<Vec<String>, AppError> {
    let tmp = temp_repo_dir();
    let repo = Repository::init(&tmp).map_err(gerr)?;
    let result = (|| -> Result<Vec<String>, AppError> {
        let mut remote = repo.remote_anonymous(url).map_err(gerr)?;
        remote.connect(Direction::Fetch).map_err(gerr)?;
        let heads = remote.list().map_err(gerr)?;
        let mut tags = Vec::new();
        for h in heads {
            let name = h.name();
            if let Some(tag) = name.strip_prefix("refs/tags/") {
                tags.push(tag.to_string());
            }
        }
        Ok(tags)
    })();
    // 及时释放句柄后清理临时目录（Windows 下先 drop 再删除）。
    drop(repo);
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{Repository, Signature};

    fn commit_file(repo: &Repository, name: &str, content: &str) -> String {
        let sig = Signature::now("t", "t@x").unwrap();
        let mut index = repo.index().unwrap();
        std::fs::write(repo.workdir().unwrap().join(name), content).unwrap();
        index.add_path(Path::new(name)).unwrap();
        let tree_id = index.write_tree().unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        let oid = repo
            .commit(Some("HEAD"), &sig, &sig, name, &tree, &parents)
            .unwrap();
        oid.to_string()
    }

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("dsh-gitops-{name}-{}", std::process::id()))
    }

    #[test]
    fn rev_parse_reads_head() {
        let dir = tmp("rev");
        let repo = Repository::init(&dir).unwrap();
        let oid = commit_file(&repo, "a.txt", "hello");
        assert_eq!(rev_parse(&dir, "HEAD").unwrap(), oid);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn behind_count_and_subject_with_remote_ref() {
        let dir = tmp("behind");
        let repo = Repository::init(&dir).unwrap();
        let c1 = commit_file(&repo, "a.txt", "one");
        let c2 = commit_file(&repo, "b.txt", "two");
        reset_hard(&dir, &c1).unwrap();
        let _ = &repo;
        let target = repo.revparse_single(&c2).unwrap().peel_to_commit().unwrap();
        repo.reference("refs/remotes/origin/main", target.id(), true, "setup")
            .unwrap();
        assert_eq!(behind_count(&dir, "HEAD", "origin/main").unwrap(), 1);
        assert_eq!(latest_subject(&dir, "origin/main").unwrap(), "b.txt");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn clean_removes_untracked_but_keeps_protected() {
        let dir = tmp("clean");
        let repo = Repository::init(&dir).unwrap();
        commit_file(&repo, "tracked.txt", "keep");
        let wd = repo.workdir().unwrap().to_path_buf();
        std::fs::write(wd.join("untracked.txt"), "x").unwrap();
        std::fs::create_dir_all(wd.join("node_modules/pkg")).unwrap();
        std::fs::write(wd.join("node_modules/pkg/index.js"), "x").unwrap();

        clean(&dir).unwrap();
        assert!(!wd.join("untracked.txt").exists());
        assert!(wd.join("tracked.txt").exists());
        assert!(wd.join("node_modules/pkg/index.js").exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
