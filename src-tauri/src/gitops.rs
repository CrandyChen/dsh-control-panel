//! 基于 git2 (libgit2) 的 git 操作封装。
//!
//! 目标：源码安装 / 更新 / 修复不再依赖外部 Git 环境，全部 git 操作在本进程内完成。
//! clone/fetch/rev-parse/behind-count/log-subject/symbolic-ref/reset/ls-remote 均由 libgit2
//! 完成；`git clean`（清理未跟踪/忽略文件）libgit2 未提供，以 worktree 遍历 + 状态标记
//! 实现（等效 `git clean -fdx`，并保留 node_modules / .venv / .git，与旧命令一致）。
//!
//! 网络错误在调用前一般已由 `net::ensure_repo_reachable` 预检，此处错误多为 git 层级问题。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use git2::build::RepoBuilder;
use git2::{FetchOptions, RemoteCallbacks, Repository, ResetType, StatusOptions};
use tauri::ipc::Channel;

use crate::detect::is_valid_repo;
use crate::error::AppError;
use crate::process::PipelineEvent;

/// GitHub 网络操作（clone / fetch / ls-remote）的最大尝试次数（国内常抽筋，多试几次）。
const GIT_NET_RETRY: u32 = 5;
/// 每次失败后的等待（毫秒），随尝试次数递增。
const GIT_NET_RETRY_DELAY_BASE_MS: u64 = 1200;
/// 克隆「连接阶段」无任何进度可等待的上限（毫秒）：超过则视为网络卡住。
const CLONE_CONNECT_TIMEOUT_MS: u64 = 90_000;
/// 克隆「传输阶段」无进度可等待的上限（毫秒）。
///
/// Windows 上 libgit2 的 HTTP 走 WinHTTP，其接收/发送/解析超时被硬编码为无限
/// （`winhttp.c` 的 `default_timeout = TIMEOUT_INFINITE`；`git2::opts` 的 server-timeout
/// 不影响它）。因此网络中途静默（连接仍在但无数据）时 libgit2 的 `recv` 会无限阻塞，
/// 必须自己加看门狗判定「长时间无进度」并中止/重试。
const CLONE_STALL_TIMEOUT_MS: u64 = 45_000;
/// 看门狗轮询间隔（毫秒）。
const CLONE_WATCHDOG_POLL_MS: u64 = 200;

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
///
/// 仅使用进程内 libgit2。为提高健壮性：
/// - 每次克隆到一个**临时目录** `<target>.partial-<n>`，成功且 `is_valid_repo` 校验通过后
///   再替换到最终 `target`，避免任何半成品 / 被放弃线程的写入污染最终目录；
/// - **看门狗**监测「长时间无进度」：Windows 上 libgit2 走 WinHTTP、接收超时为无限，网络
///   静默时库会永久阻塞，故由看门狗判定卡顿 → 放弃该次克隆线程 → 换新临时目录重试。
pub fn clone_with_progress(
    url: &str,
    target: &Path,
    channel: &Channel<PipelineEvent>,
    step: &str,
) -> Result<(), AppError> {
    let mut last_err: Option<AppError> = None;
    for attempt in 0..GIT_NET_RETRY {
        // 清理历史残留（含上次可能被放弃线程留下的 .partial-*）。
        clean_stale_partials(target);
        let work = temp_clone_dir(target, attempt);
        let _ = std::fs::remove_dir_all(&work);

        match clone_once_with_timeout(url, &work, channel, step) {
            CloneOutcome::Ok => {
                let _ = std::fs::remove_dir_all(target);
                if std::fs::rename(&work, target).is_ok() {
                    // 换名成功后清理其它残留临时目录（best-effort）。
                    clean_stale_partials(target);
                    return Ok(());
                }
                last_err = Some(AppError::Git("克隆完成后整理目录失败".into()));
                let _ = std::fs::remove_dir_all(&work);
            }
            CloneOutcome::Invalid => {
                last_err = Some(AppError::Git(
                    "源码克隆不完整（工作树校验失败，package.json 缺失）".into(),
                ));
                let _ = std::fs::remove_dir_all(&work);
            }
            CloneOutcome::Stalled => {
                last_err = Some(AppError::Git("克隆多次因网络卡顿失败（长时间无进度）".into()));
                let _ = std::fs::remove_dir_all(&work);
            }
            CloneOutcome::Err(e) => {
                last_err = Some(e);
                let _ = std::fs::remove_dir_all(&work);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(
            GIT_NET_RETRY_DELAY_BASE_MS * (attempt as u64 + 1),
        ));
    }
    clean_stale_partials(target);
    Err(last_err.unwrap_or_else(|| AppError::Git("克隆仓库失败".into())))
}

/// 本次克隆的临时目录：`<target>.partial-<attempt>`（与最终 target 同父目录，便于成功后 rename）。
fn temp_clone_dir(target: &Path, attempt: u32) -> PathBuf {
    let base = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "clone".to_string());
    target
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{base}.partial-{attempt}"))
}

/// 清理 target 旁的 `<base>.partial-*` 残留临时目录（best-effort）。
fn clean_stale_partials(target: &Path) {
    let Some(parent) = target.parent() else { return };
    let base = format!(
        "{}.partial-",
        target
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    );
    if let Ok(rd) = std::fs::read_dir(parent) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with(&base) {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
}

/// 单次克隆结果：成功 / 校验不通过 / 看门狗判定卡顿 / 普通错误。
enum CloneOutcome {
    Ok,
    Invalid,
    Stalled,
    Err(AppError),
}

/// 单次（无重试）克隆，带**看门狗超时**。
///
/// libgit2 的 clone/传输是同步阻塞的，无法在另一端主动停止；看门狗运行在调用线程：
/// - 克隆在**子线程**执行，`transfer_progress` 更新「最近有进度的时间戳」，并按百分比节流
///   推送进度；同时检查中止标志（为 true 则返回 false，让库尽快中止）；
/// - 若「连接阶段超过 `CLONE_CONNECT_TIMEOUT_MS` 仍无任何进度」或「传输阶段超过
///   `CLONE_STALL_TIMEOUT_MS` 无进度」→ 判定卡顿：置中止标志；若库仍在回调中则它能立即退出；
///   若库永久阻塞在网络读上（不回调），则**放弃该线程（不 join）**，由调用方清理临时目录后
///   重试——该阻塞线程随进程退出而终止，不影响本次流程。
fn clone_once_with_timeout(
    url: &str,
    target: &Path,
    channel: &Channel<PipelineEvent>,
    step: &str,
) -> CloneOutcome {
    let abort = Arc::new(AtomicBool::new(false));
    let last_activity = Arc::new(Mutex::new(Instant::now()));
    let first_progress = Arc::new(AtomicBool::new(false));
    let transfer_done = Arc::new(AtomicBool::new(false));
    let started = Instant::now();

    let ch = channel.clone();
    let step_s = step.to_string();
    let abort2 = abort.clone();
    let la2 = last_activity.clone();
    let fp2 = first_progress.clone();
    let td2 = transfer_done.clone();
    let last_pct = Arc::new(Mutex::new(-1i32));

    let target_owned = target.to_path_buf();
    let url_owned = url.to_string();
    let handle = std::thread::spawn(move || {
        // 收集回调所需句柄，放进一个具名作用域以配合闭包捕获（避免 move 冲突）。
        let abort = abort2;
        let la = la2;
        let fp = fp2;
        let td = td2;
        let lp = last_pct;
        let ch = ch;
        let step_s = step_s;
        let mut cb = RemoteCallbacks::new();
        cb.transfer_progress(move |p| {
            let now = Instant::now();
            *la.lock().unwrap() = now;
            fp.store(true, Ordering::Relaxed);
            let received = p.received_objects() as u64;
            let total = p.total_objects() as u64;
            if received > 0 && total > 0 && received >= total {
                td.store(true, Ordering::Relaxed);
            }
            if abort.load(Ordering::Relaxed) {
                return false;
            }
            // 按百分比节流推送进度（降低 IPC 压力，也缓解等待前端消费的回压）。
            if total > 0 {
                let pct = ((received as f64 / total as f64) * 100.0) as i32;
                let mut guard = lp.lock().unwrap();
                if *guard != pct {
                    *guard = pct;
                    let (recv, tot) = (received, total);
                    drop(guard);
                    let _ = ch.send(PipelineEvent::DownloadProgress {
                        step: step_s.clone(),
                        received: recv,
                        total: tot,
                        speed_bps: 0,
                    });
                }
            }
            true
        });
        let mut fo = FetchOptions::new();
        fo.remote_callbacks(cb);
        let mut builder = RepoBuilder::new();
        builder.fetch_options(fo);
        builder.clone(&url_owned, &target_owned).map_err(gerr)
    });

    loop {
        if handle.is_finished() {
            return match handle.join() {
                Ok(Ok(_)) if is_valid_repo(target) => CloneOutcome::Ok,
                Ok(Ok(_)) => CloneOutcome::Invalid,
                Ok(Err(e)) => CloneOutcome::Err(AppError::Git(e.to_string())),
                Err(_) => CloneOutcome::Err(AppError::Git("克隆线程异常退出".into())),
            };
        }
        let now = Instant::now();
        if !transfer_done.load(Ordering::Relaxed) {
            let timed_out = if !first_progress.load(Ordering::Relaxed) {
                now.duration_since(started) > Duration::from_millis(CLONE_CONNECT_TIMEOUT_MS)
            } else {
                let last = *last_activity.lock().unwrap();
                now.duration_since(last) > Duration::from_millis(CLONE_STALL_TIMEOUT_MS)
            };
            if timed_out {
                // 判定卡顿：请求中止（库在回调中可立即退出）；随后放弃该线程并返回 Stalled。
                abort.store(true, Ordering::Relaxed);
                // 给库一个短暂机会自行中止后退出；若仍阻塞则直接弃置（不 join）。
                std::thread::sleep(Duration::from_millis(300));
                return CloneOutcome::Stalled;
            }
        }
        std::thread::sleep(Duration::from_millis(CLONE_WATCHDOG_POLL_MS));
    }
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

/// 读取指定 rev（如 `origin/main`）处某个 blob 的文本内容（`git show <rev>:<path>`）。
pub fn show_file(dir: &Path, rev: &str, path: &str) -> Result<String, AppError> {
    let repo = open_repo(dir)?;
    let obj = repo.revparse_single(rev).map_err(gerr)?;
    let tree = obj.peel_to_tree().map_err(gerr)?;
    let entry = tree
        .get_path(Path::new(path))
        .map_err(gerr)?;
    let blob = repo.find_blob(entry.id()).map_err(gerr)?;
    String::from_utf8(blob.content().to_vec()).map_err(|e| {
        AppError::Git(format!("文件 {path} 不是有效的 UTF-8 文本：{e}"))
    })
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

/// 生成一个唯一的临时目录（用于远程仓库探测 / 读取临时 git 句柄）。
fn temp_repo_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("dsh-gitops-ls-{}-{}", std::process::id(), nanos))
}

/// 读取远程某路径文件的文本 + 对应分支 HEAD 的提交 SHA（进程内 libgit2，不依赖系统 git）。
/// 用于「插件检查更新」读取 git 依赖 HEAD 的 `package.json` 以确定可更新到的版本/提交。
pub fn read_remote_file_with_head(
    url: &str,
    path: &str,
    branch: Option<&str>,
) -> Result<(String, String), AppError> {
    let tmp = temp_repo_dir();
    let _ = std::fs::remove_dir_all(&tmp);
    let repo = Repository::init_bare(&tmp).map_err(gerr)?;
    let result = (|| -> Result<(String, String), AppError> {
        let mut fo = FetchOptions::new();
        // 仅网络 URL 浅取一行（缩小传输）；本地路径（测试 / 本地仓库）不支持 shallow。
        if url.starts_with("http://") || url.starts_with("https://") {
            fo.depth(1);
        }
        let mut remote = repo.remote("origin", url).map_err(gerr)?;
        remote
            .fetch(
                &["+refs/heads/*:refs/remotes/origin/*"],
                Some(&mut fo),
                None,
            )
            .map_err(gerr)?;
        let branch = match branch {
            Some(b) if !b.trim().is_empty() => b.to_string(),
            _ => default_branch(&tmp)?,
        };
        let rev = format!("origin/{branch}");
        let text = show_file(&tmp, &rev, path)?;
        let commit = rev_parse(&tmp, &rev)?;
        Ok((text, commit))
    })();
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

    #[test]
    fn temp_clone_dir_naming() {
        let target = Path::new("C:/repo/deepseek-harness");
        let d = temp_clone_dir(target, 2);
        assert_eq!(
            d.file_name().unwrap().to_string_lossy(),
            "deepseek-harness.partial-2"
        );
        assert_eq!(d.parent(), target.parent());
    }

    #[test]
    fn clean_stale_partials_keeps_target_and_other_dirs() {
        let parent = tmp("partials");
        let _ = std::fs::remove_dir_all(&parent);
        std::fs::create_dir_all(&parent).unwrap();
        let target = parent.join("deepseek-harness");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::create_dir_all(parent.join("deepseek-harness.partial-0")).unwrap();
        std::fs::create_dir_all(parent.join("deepseek-harness.partial-3")).unwrap();
        std::fs::create_dir_all(parent.join("other")).unwrap();

        clean_stale_partials(&target);

        assert!(target.exists());
        assert!(!parent.join("deepseek-harness.partial-0").exists());
        assert!(!parent.join("deepseek-harness.partial-3").exists());
        assert!(parent.join("other").exists());
        std::fs::remove_dir_all(&parent).ok();
    }

    #[test]
    fn read_remote_default_file_reads_version_from_local_repo() {
        // 用进程内 libgit2（而非系统 git）从本地仓库读取 package.json 的版本。
        let src = tmp("remotefile-src");
        let _ = std::fs::remove_dir_all(&src);
        let repo = Repository::init(&src).unwrap();
        let branch = repo
            .head()
            .ok()
            .and_then(|h| h.shorthand().map(String::from))
            .unwrap_or_else(|| "master".into());
        commit_file(&repo, "package.json", r#"{"name":"x","version":"9.9.9"}"#);
        let (text, head_commit) =
            read_remote_file_with_head(&src.to_string_lossy(), "package.json", Some(&branch)).unwrap();
        assert_eq!(head_commit, crate::gitops::rev_parse(&src, "HEAD").unwrap());
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["version"].as_str(), Some("9.9.9"));
        std::fs::remove_dir_all(&src).ok();
    }
}
