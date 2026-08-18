//! 卸载：枚举「安装目录 + DSH 用户数据目录(~/.dsh)」形成清单，
//! 用户勾选确认后逐条删除。删除前强制校验路径来源与安全性。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::AppHandle;

use crate::config;
use crate::detect::dsh_home;
use crate::error::AppError;
use crate::logging::Logger;
use crate::process::{PipelineEvent};

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UninstallEntry {
    pub id: String,
    pub name: String,
    pub path: String,
    pub kind: String,
    pub size: u64,
    pub items: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UninstallPreview {
    pub entries: Vec<UninstallEntry>,
    pub install_dir: Option<String>,
    pub dsh_home: String,
}

/// 统计路径大小与条目数（不跟随符号链接；限制深度与条目数防止卡死）。
pub fn stat_path(p: &Path) -> (u64, u64) {
    fn walk(p: &Path, size: &mut u64, items: &mut u64, depth: u32) {
        if depth > 48 || *items > 200_000 {
            return;
        }
        match std::fs::symlink_metadata(p) {
            Ok(md) if md.file_type().is_symlink() || md.is_file() => {
                *size += md.len();
                *items += 1;
            }
            Ok(md) if md.is_dir() => {
                *items += 1;
                if let Ok(rd) = std::fs::read_dir(p) {
                    for e in rd.flatten() {
                        walk(&e.path(), size, items, depth + 1);
                    }
                }
            }
            _ => {}
        }
    }
    let mut size = 0;
    let mut items = 0;
    walk(p, &mut size, &mut items, 0);
    (size, items)
}

/// 生成卸载预览清单条目：安装目录（整棵）+ DSH 用户数据目录（整棵），各为一项。
/// 目录不存在时对应条目省略；返回值纯函数，便于测试。
pub fn build_preview_entries(install_dir: Option<&str>, dsh_home: &str) -> Vec<UninstallEntry> {
    let mut entries = Vec::new();

    if let Some(dir) = install_dir {
        let p = Path::new(dir);
        if p.exists() {
            let (size, items) = stat_path(p);
            entries.push(UninstallEntry {
                id: "install".into(),
                name: format!("DeepSeek Harness 安装目录（{dir}）"),
                path: dir.to_string(),
                kind: "directory".into(),
                size,
                items,
            });
        }
    }

    let hp = Path::new(dsh_home);
    if hp.exists() {
        let (size, items) = stat_path(hp);
        entries.push(UninstallEntry {
            id: "dsh-home".into(),
            name: format!("DSH 用户数据目录（{dsh_home}）"),
            path: dsh_home.to_string(),
            kind: "directory".into(),
            size,
            items,
        });
    }

    entries
}

/// 生成卸载预览清单：安装目录 + DSH 用户数据目录，两项粗粒度待选。
pub fn build_preview(app: &AppHandle) -> UninstallPreview {
    let cfg = config::load_config(app);
    let home = dsh_home();
    let entries = build_preview_entries(cfg.install_dir.as_deref(), &home);
    UninstallPreview {
        entries,
        install_dir: cfg.install_dir.clone(),
        dsh_home: home,
    }
}

/// 安全护栏：拒绝删除盘符根、Windows 目录、用户主目录本身等危险路径。
pub fn is_forbidden_path(p: &Path) -> bool {
    let s = p.to_string_lossy().to_lowercase();
    if s.len() <= 3 {
        return true;
    }
    for d in b'a'..=b'z' {
        let root = format!("{}:\\", d as char);
        if s == root || s.starts_with(&format!("{}windows", root)) {
            return true;
        }
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        if !home.is_empty() && s == home.to_lowercase() {
            return true;
        }
    }
    false
}

/// 执行卸载：校验每个路径都在预览清单内且安全，然后逐条删除。
pub fn uninstall(
    app: &AppHandle,
    selected: Vec<String>,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<(), String> {
    // 先停止 web 服务，避免文件占用。
    let _ = crate::web::stop_web(app);

    let preview = build_preview(app);
    let allowed: HashSet<&str> = preview.entries.iter().map(|e| e.path.as_str()).collect();

    for sel in &selected {
        if !allowed.contains(sel.as_str()) {
            return Err(AppError::NotInPreview(sel.clone()).friendly());
        }
        if is_forbidden_path(Path::new(sel)) {
            return Err(AppError::InvalidPath(sel.clone()).friendly());
        }
    }

    for sel in &selected {
        let p = PathBuf::from(sel);
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| sel.clone());
        let _ = channel.send(PipelineEvent::StepStarted {
            id: "delete".into(),
            title: format!("删除 {name}"),
        });
        logger.info(&format!("🗑 删除 {sel}"));

        let is_dir = std::fs::symlink_metadata(&p)
            .map(|md| md.is_dir())
            .unwrap_or(false);
        let result = if is_dir {
            std::fs::remove_dir_all(&p)
        } else {
            std::fs::remove_file(&p)
        };
        match result {
            Ok(()) => {
                let _ = channel.send(PipelineEvent::StepFinished {
                    id: "delete".into(),
                    exit_code: 0,
                });
            }
            Err(e) => {
                let msg = format!("删除 {sel} 失败：{e}。请关闭占用该路径的程序（如终端、编辑器）后重试。");
                let _ = channel.send(PipelineEvent::Error { message: msg.clone() });
                logger.error(&msg);
                return Err(msg);
            }
        }
    }

    let _ = channel.send(PipelineEvent::Finished { ok: true });

    let mut cfg = config::load_config(app);
    cfg.install_dir = None;
    cfg.installed_version = None;
    cfg.installed_commit = None;
    cfg.last_updated_at = None;
    cfg.update_available = false;
    cfg.latest_commit = None;
    cfg.latest_subject = None;
    config::save_config(app, &cfg).map_err(|e| e)?;

    logger.info("✅ 卸载完成");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forbidden_paths_are_rejected() {
        assert!(is_forbidden_path(Path::new("C:\\")));
        assert!(is_forbidden_path(Path::new("C:\\Windows")));
        assert!(is_forbidden_path(Path::new("D:\\")));
        assert!(is_forbidden_path(Path::new("C:\\")));
    }

    #[test]
    fn normal_paths_are_allowed() {
        assert!(!is_forbidden_path(Path::new("D:\\dev\\deepseek-harness")));
        assert!(!is_forbidden_path(Path::new("C:\\Users\\crandy\\.dsh\\profiles")));
    }

    #[test]
    fn stat_path_counts_files() {
        let tmp = std::env::temp_dir().join(format!("dsh-stat-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("a/b")).unwrap();
        std::fs::write(tmp.join("a/one.txt"), "hello").unwrap();
        std::fs::write(tmp.join("a/b/two.txt"), "world!").unwrap();
        let (size, items) = stat_path(&tmp);
        assert_eq!(size, 11);
        assert_eq!(items, 5); // 根 + a + one.txt + b + two.txt
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    // ---------- 卸载清单颗粒度（粗粒度：安装目录 + 用户数据目录各一项） ----------

    #[test]
    fn preview_lists_install_dir_and_dsh_home_as_two_entries() {
        let base = std::env::temp_dir().join(format!("dsh-uninstall-{}", std::process::id()));
        let install = base.join("deepseek-harness");
        let home = base.join(".dsh");
        std::fs::create_dir_all(install.join("apps/web")).unwrap();
        std::fs::create_dir_all(home.join("profiles/web")).unwrap();
        std::fs::write(install.join("package.json"), "{}").unwrap();
        std::fs::write(home.join("profiles/web/package.json"), "{}").unwrap();

        let entries = build_preview_entries(
            Some(install.to_string_lossy().as_ref()),
            &home.to_string_lossy(),
        );
        assert_eq!(entries.len(), 2, "应只有安装目录与用户数据目录两项");
        assert_eq!(entries[0].id, "install");
        assert!(entries[0].path.contains("deepseek-harness"));
        assert_eq!(entries[1].id, "dsh-home");
        assert!(entries[1].path.ends_with(".dsh"));

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn preview_omits_missing_paths() {
        let base = std::env::temp_dir().join(format!("dsh-uninstall2-{}", std::process::id()));
        let install = base.join("deepseek-harness");
        std::fs::create_dir_all(&install).unwrap();

        // 用户数据目录不存在：只剩安装目录一项。
        let entries = build_preview_entries(
            Some(install.to_string_lossy().as_ref()),
            &base.join(".dsh").to_string_lossy(),
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "install");

        // 两者都不存在：空清单。
        let none = build_preview_entries(
            Some("Z:\\definitely-not-exist-xyz"),
            "Z:\\no-dsh-xyz",
        );
        assert!(none.is_empty());

        std::fs::remove_dir_all(&base).ok();
    }
}
