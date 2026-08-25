//! 插件管理：读取 profile 清单、智能解析安装输入、执行 `dsh plugin` 命令。
//!
//! 命令语义（与 DeepSeek Harness apps/cli 的 `dsh plugin` 一致）：
//! `dsh plugin --profile <name> <pnpm 子命令与参数>` 是纯 pnpm 转发器，在
//! `$DSH_HOME/profiles/<name>` 目录内执行 pnpm。全局 `dsh` 不可识别时改用
//! `pnpm dsh plugin ...`（cwd = 安装目录）。插件列表直接读取 profile 清单
//! （dependencies + dsh.profile.bundles），比解析 `pnpm list` 输出更可靠；
//! github 插件的 remove/update 标识 = 清单里记录的依赖 key，原样复用。
//!
//! pnpm ≥10 会拦截依赖构建脚本（含 git 托管插件的 prepare 脚本，报
//! ERR_PNPM_GIT_DEP_PREPARE_NOT_ALLOWED）；失败重试前自动从输出解析被拦截的
//! 包（含 pnpm 提示的精确 allowBuilds key），写入该 profile 的
//! pnpm-workspace.yaml（显式 `包名: true`；实测 `"*": true` 通配不会放行）后重试。

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::AppHandle;

use crate::config::{self, AppConfig};
use crate::error::AppError;
use crate::logging::Logger;
use crate::process::{no_window, spawn_any, PipelineEvent};

/// 失败分析保留的输出行数（错误上下文用）。
const MAX_KEPT_LINES: usize = 40;

// ─────────────────────────────── 数据模型 ───────────────────────────────

/// 单个插件条目（来自 profile 清单的 dependencies）。
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PluginEntry {
    /// 依赖 key（pnpm 记录的精确标识，更新 / 卸载时原样复用）。
    pub key: String,
    /// 依赖 value（版本范围或 git spec）。
    pub spec: String,
    /// 是否在 dsh.profile.bundles 激活层栈中（组合包插件）。
    pub is_bundle: bool,
    /// 已安装的实际版本（读 node_modules/<key>/package.json 的 version；
    /// 未安装 / 读取失败为 None，GitHub 插件据此显示真实版本号而非 git spec）。
    pub version: Option<String>,
}

/// 指定 profile 的插件列表。
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PluginList {
    pub profile: String,
    pub profile_dir: String,
    pub entries: Vec<PluginEntry>,
    /// 内置组合包（在 bundles 中但非依赖，随 dsh 安装提供，只读不可卸载）。
    pub builtin_bundles: Vec<String>,
    /// profile 是否已初始化（存在 package.json）。
    pub initialized: bool,
    /// 当前是否使用 `pnpm dsh` 执行命令。
    pub use_pnpm_dsh: bool,
}

/// 插件操作结果（成功 / 失败摘要，详情走 Channel 事件流）。
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PluginOpResult {
    pub ok: bool,
    pub message: String,
    pub action: String,
}

/// 安装输入解析结果。
pub struct ParsedAdd {
    /// 规范化后的插件标识（可多个）。
    pub specs: Vec<String>,
    /// 完整命令中提取的 --profile（覆盖对话框当前 profile）。
    pub profile: Option<String>,
}

// ─────────────────────────────── 目录与清单 ───────────────────────────────

/// 读取已安装包的实际版本（`node_modules/<key>/package.json` 的 version 字段）。
/// pnpm 的 node_modules 下包是符号链接，`read_to_string` 可正常跟随；
/// 未安装 / 文件缺失 / 解析失败返回 None。
pub fn installed_version(dir: &Path, key: &str) -> Option<String> {
    let nm = dir.join("node_modules");
    let pkg_dir = match key.split_once('/') {
        // scoped 包：@scope/name → node_modules/@scope/name（scope 已含前导 @）。
        Some((scope, name)) => nm.join(scope).join(name),
        None => nm.join(key),
    };
    let text = std::fs::read_to_string(pkg_dir.join("package.json")).ok()?;
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()?
        .get("version")
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// 校验 profile 名称并返回其目录（`$DSH_HOME/profiles/<name>`），
/// 规则与 dsh 的 resolveProfileDir 一致。
pub fn profile_dir(profile: &str) -> Result<PathBuf, String> {
    let profile = profile.trim();
    if profile.is_empty()
        || profile == "."
        || profile == ".."
        || profile == "node_modules"
        || profile.contains('/')
        || profile.contains('\\')
    {
        return Err(crate::i18n::t_fmt("plugin.profile.invalid", &[profile]));
    }
    Ok(PathBuf::from(crate::detect::dsh_home())
        .join("profiles")
        .join(profile))
}

/// 列出指定 profiles 目录下的 profile 名称（仅目录、忽略隐藏项、排序）。
/// 目录不存在 / 无子目录返回空。供插件管理的 profile 下拉框使用。
pub fn list_profiles_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| !n.is_empty() && !n.starts_with('.'))
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}

/// 列出 `$DSH_HOME/profiles` 下已存在的 profile（供下拉框选择）。
pub fn list_profiles() -> Vec<String> {
    list_profiles_in(&PathBuf::from(crate::detect::dsh_home()).join("profiles"))
}

/// 读取 profile 清单并整理为插件列表（依赖 + 内置组合包，按名称排序）。
pub fn read_plugin_list(dir: &Path, profile: &str, use_pnpm_dsh: bool) -> PluginList {
    let mut entries = Vec::new();
    let mut builtin_bundles = Vec::new();
    let initialized = dir.join("package.json").is_file();

    if let Ok(text) = std::fs::read_to_string(dir.join("package.json")) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            let bundles: Vec<String> = v
                .pointer("/dsh/profile/bundles")
                .and_then(|b| b.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            if let Some(deps) = v.get("dependencies").and_then(|d| d.as_object()) {
                for (key, val) in deps {
                    entries.push(PluginEntry {
                        key: key.clone(),
                        spec: val.as_str().map(String::from).unwrap_or_default(),
                        is_bundle: bundles.contains(key),
                        version: installed_version(dir, key),
                    });
                }
            }
            for b in &bundles {
                if !entries.iter().any(|e| &e.key == b) {
                    builtin_bundles.push(b.clone());
                }
            }
        }
    }
    entries.sort_by(|a, b| a.key.cmp(&b.key));

    PluginList {
        profile: profile.to_string(),
        profile_dir: dir.to_string_lossy().to_string(),
        entries,
        builtin_bundles,
        initialized,
        use_pnpm_dsh,
    }
}

// ─────────────────────────────── profile bundle 校准 ───────────────────────────────

/// bundle 包是否可解析：内核 `node_modules/<pkg>`，或沿 profile 目录上溯
/// （profile 自身 node_modules / pnpm workspace 上溯到 profiles/node_modules、
/// `$DSH_HOME/node_modules`）。与 dsh-app-boot 的 `resolveBundleDir` 一致。
fn bundle_resolvable(install_dir: &Path, profile_dir: &Path, package: &str) -> bool {
    if install_dir
        .join("node_modules")
        .join(package)
        .join("package.json")
        .is_file()
    {
        return true;
    }
    let mut dir = Some(profile_dir);
    while let Some(d) = dir {
        if d.join("node_modules").join(package).join("package.json").is_file() {
            return true;
        }
        dir = d.parent();
    }
    false
}

/// 校准指定 home 下所有已初始化 profile 的 bundle 列表：移除当前内核（或 profile
/// 自身依赖）无法解析的 bundle 条目，使 profile 与已安装内核对齐。
///
/// 背景：`~/.dsh` 可能由其它 DSH 桌面发行版写入（如带 `dsh-tauri` 自定义 bundle
/// 的 profile），换成纯预构建内核后 `dsh web` 会因无法解析该 bundle 启动即失败
/// （`dsh: cannot resolve profile bundle ...`）。此函数为预构建模式的
/// 安装 / 更新 / 修复 / 启动前调用，保证启动可用；只移除无法解析的条目，
/// 保留 profile 的其它配置与插件。
///
/// 返回被移除的 (profile 名, 被移除的 bundle 列表)；单个 profile 读取/写回失败
/// 跳过（不阻断主流程）。
pub fn reconcile_profile_bundles(
    install_dir: &Path,
    home: &Path,
) -> Vec<(String, Vec<String>)> {
    let profiles = home.join("profiles");
    let mut removed_all: Vec<(String, Vec<String>)> = Vec::new();
    let Ok(rd) = std::fs::read_dir(&profiles) else {
        return removed_all;
    };
    for e in rd.flatten() {
        let pdir = e.path();
        if !pdir.is_dir() {
            continue;
        }
        let Ok(name) = e.file_name().into_string() else {
            continue;
        };
        if name.is_empty() || name.starts_with('.') {
            continue;
        }
        let manifest_path = pdir.join("package.json");
        let Ok(text) = std::fs::read_to_string(&manifest_path) else {
            continue;
        };
        let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let (before, kept) = {
            let Some(bundles) = v
                .pointer("/dsh/profile/bundles")
                .and_then(|b| b.as_array())
            else {
                continue;
            };
            let before: Vec<String> = bundles
                .iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect();
            if before.is_empty() {
                continue;
            }
            let kept: Vec<serde_json::Value> = bundles
                .iter()
                .filter(|x| {
                    x.as_str()
                        .map(|pkg| bundle_resolvable(install_dir, &pdir, pkg))
                        .unwrap_or(true)
                })
                .cloned()
                .collect();
            (before, kept)
        };
        if kept.len() == before.len() {
            continue; // 全部可解析，无需改动
        }
        let removed: Vec<String> = before
            .iter()
            .filter(|pkg| !kept.iter().any(|k| k.as_str() == Some(pkg.as_str())))
            .cloned()
            .collect();
        if let Some(arr) = v
            .pointer_mut("/dsh/profile/bundles")
            .and_then(|b| b.as_array_mut())
        {
            *arr = kept;
        }
        // 与 dsh-app-boot 的写回格式一致（2 空格缩进 + 末尾换行）。
        let Ok(json) = serde_json::to_string_pretty(&v) else {
            continue;
        };
        if std::fs::write(&manifest_path, format!("{json}\n")).is_err() {
            continue;
        }
        removed_all.push((name, removed));
    }
    removed_all
}

/// 使用默认 `$DSH_HOME` 校准全部 profile 的 bundle 列表。
pub fn reconcile_all_profiles(install_dir: &Path) -> Vec<(String, Vec<String>)> {
    reconcile_profile_bundles(install_dir, Path::new(&crate::detect::dsh_home()))
}

// ─────────────────────────────── 智能输入解析 ───────────────────────────────

/// 剥离输入开头的 PowerShell 环境变量赋值（如 `$env:PNPM_ALLOW_BUILDS="*";`）。
fn strip_env_prefix(mut s: &str) -> &str {
    loop {
        let t = s.trim_start();
        if !t.to_ascii_lowercase().starts_with("$env:") {
            return t;
        }
        let Some(qs) = t.find('"').or_else(|| t.find('\'')) else {
            return t;
        };
        let quote = t[qs..].chars().next().unwrap();
        let Some(rel) = t[qs + 1..].find(quote) else {
            return t;
        };
        let ce = qs + 1 + rel;
        let after = t[ce + 1..].trim_start();
        let after = after.strip_prefix(';').map(str::trim_start).unwrap_or(after);
        if after.len() >= t.len() {
            return after;
        }
        s = after;
    }
}

/// 是否为 pnpm 可直接按「远程压缩包」安装的 URL：
/// 以 `.tar.gz` / `.tgz` 结尾（GitHub 归档 / release 附件），或含 `/archive/` 路径段
/// （GitHub 归档链接，如 `…/archive/refs/tags/v0.6.3.tar.gz`，部分插件官方文档直接给出）。
/// 命中时原样透传给 pnpm，不做 `github:owner/repo` 转换。
fn is_tarball_url(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    l.ends_with(".tar.gz") || l.ends_with(".tgz") || l.contains("/archive/")
}

/// 从 GitHub 链接路径段（owner/repo[.git][/…][#ref]）构造 `github:owner/repo[#ref]`。
fn github_from_path(rest: &str, original: &str) -> Result<String, String> {
    let (path_part, ref_part) = match rest.split_once('#') {
        Some((p, r)) => (p, Some(r.to_string())),
        None => (rest, None),
    };
    let mut segs: Vec<&str> = path_part.split('/').filter(|s| !s.is_empty()).collect();
    if segs.len() < 2 {
        return Err(crate::i18n::t_fmt("plugin.github.bad.link", &[original]));
    }
    let owner = segs.remove(0);
    let repo = segs.remove(0).trim_end_matches(".git");
    if owner.is_empty() || repo.is_empty() {
        return Err(crate::i18n::t_fmt("plugin.github.missing.owner", &[original]));
    }
    if !segs.is_empty() {
        return Err(crate::i18n::t("plugin.github.inner.path"));
    }
    let mut out = format!("github:{owner}/{repo}");
    if let Some(r) = ref_part {
        if r.is_empty() {
            return Err(crate::i18n::t_fmt("plugin.github.empty.ref", &[original]));
        }
        out.push('#');
        out.push_str(&r);
    }
    Ok(out)
}

/// 校验并规范化单个插件标识。
fn classify_spec(raw: &str) -> Result<String, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(crate::i18n::t("plugin.spec.empty"));
    }
    if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(crate::i18n::t_fmt("plugin.spec.invalid.char", &[s]));
    }
    let lower = s.to_ascii_lowercase();
    if let Some(rest) = lower
        .strip_prefix("https://github.com/")
        .or_else(|| lower.strip_prefix("http://github.com/"))
    {
        // GitHub 压缩包链接（归档 / release 附件）：pnpm 直接按远程 tarball 安装，
        // 原样透传（部分插件官方文档的安装命令就是这种链接）。
        if is_tarball_url(s) {
            return Ok(s.to_string());
        }
        return github_from_path(rest, s);
    }
    if let Some(rest) = s.strip_prefix("github:") {
        let (path_part, ref_part) = match rest.split_once('#') {
            Some((p, r)) => (p, Some(r)),
            None => (rest, None),
        };
        if path_part.is_empty() || path_part.split('/').count() != 2 {
            return Err(crate::i18n::t_fmt("plugin.github.bad.spec", &[s]));
        }
        if let Some(r) = ref_part {
            if r.is_empty() {
                return Err(crate::i18n::t_fmt("plugin.spec.hash.empty", &[s]));
            }
        }
        return Ok(s.to_string());
    }
    if s.starts_with("git+")
        || s.starts_with("git@")
        || s.starts_with("ssh://")
        || s.starts_with("file:")
        || s.starts_with("link:")
        || s.starts_with("https://")
        || s.starts_with("http://")
    {
        // pnpm 支持的 git / 本地 spec，原样透传。
        return Ok(s.to_string());
    }
    if !s.starts_with('@') && s.contains('@') {
        // pkg@版本 形式：name 部分必须是不含 / 的包名。
        let name = s.split('@').next().unwrap_or("");
        if name.is_empty() || name.contains('/') {
            return Err(crate::i18n::t_fmt("plugin.spec.unrecognized", &[s]));
        }
        return Ok(s.to_string());
    }
    if s.starts_with('@') {
        // @scope/name[@版本]
        let core = s.split('@').nth(1).unwrap_or("");
        let name = core.split('/').next().unwrap_or("");
        let rest_after = core.split('/').nth(1).unwrap_or("");
        let rest_after = rest_after.split('@').next().unwrap_or("");
        if name.is_empty() || rest_after.is_empty() {
            return Err(crate::i18n::t_fmt("plugin.npm.bad.scope", &[s]));
        }
        return Ok(s.to_string());
    }
    if s.contains('/') {
        // owner/repo[#ref] GitHub 简写（pnpm 原生支持）。
        let (path_part, ref_part) = match s.split_once('#') {
            Some((p, r)) => (p, Some(r)),
            None => (s, None),
        };
        let segs: Vec<&str> = path_part.split('/').collect();
        if segs.len() != 2 || segs[0].is_empty() || segs[1].is_empty() {
            return Err(crate::i18n::t_fmt("plugin.spec.ownerrepo.bad", &[s]));
        }
        if let Some(r) = ref_part {
            if r.is_empty() {
                return Err(crate::i18n::t_fmt("plugin.spec.hash.empty", &[s]));
            }
        }
        return Ok(s.to_string());
    }
    if s.contains('#') {
        return Err(crate::i18n::t_fmt("plugin.spec.unrecognized", &[s]));
    }
    // 裸 npm 包名。
    Ok(s.to_string())
}

/// 解析安装框输入：npm 包名 / github: 标识 / GitHub 链接 / git+ 等 spec /
/// 完整 `dsh plugin … add …` 或 `pnpm dsh plugin … add …` 命令。
pub fn parse_add_input(input: &str) -> Result<ParsedAdd, String> {
    let s = strip_env_prefix(input.trim());
    if s.is_empty() {
        return Err(crate::i18n::t("plugin.input.empty"));
    }
    let tokens: Vec<&str> = s.split_whitespace().collect();
    let first = tokens[0].to_ascii_lowercase();
    if first == "dsh"
        || first == "dsh.cmd"
        || first == "pnpm"
        || first == "pnpm.cmd"
        || first == "npx"
        || first == "npm"
        || first == "yarn"
    {
        return parse_command_form(&tokens);
    }
    if tokens.len() > 1 {
        return Err(crate::i18n::t_fmt("plugin.input.unrecognized", &[s]));
    }
    Ok(ParsedAdd {
        specs: vec![classify_spec(tokens[0])?],
        profile: None,
    })
}

/// 解析完整命令形式（tokens 首词为 dsh / pnpm 等）。
fn parse_command_form(tokens: &[&str]) -> Result<ParsedAdd, String> {
    let mut i = 0;
    // 跳过包管理器前缀：pnpm / pnpm.cmd / npm / npx / yarn / exec / dlx。
    while i < tokens.len() {
        let t = tokens[i].to_ascii_lowercase();
        match t.as_str() {
            "pnpm" | "pnpm.cmd" | "npm" | "npx" | "yarn" | "exec" | "dlx" => i += 1,
            _ => break,
        }
    }
    if i < tokens.len()
        && (tokens[i].eq_ignore_ascii_case("dsh") || tokens[i].eq_ignore_ascii_case("dsh.cmd"))
    {
        i += 1;
    }
    if i < tokens.len() && tokens[i].eq_ignore_ascii_case("plugin") {
        i += 1;
    }
    // 扫描操作动词：跳过 --profile <name> 与其他 flag。
    let mut profile: Option<String> = None;
    let mut verb: Option<&str> = None;
    while i < tokens.len() {
        let t = tokens[i];
        if let Some(v) = t.strip_prefix("--profile=") {
            if v.is_empty() {
                return Err(crate::i18n::t("plugin.cmd.missing.profile"));
            }
            profile = Some(v.to_string());
            i += 1;
        } else if t == "--profile" {
            i += 1;
            if i >= tokens.len() {
                return Err(crate::i18n::t("plugin.cmd.missing.profile"));
            }
            profile = Some(tokens[i].to_string());
            i += 1;
        } else if t.starts_with('-') {
            i += 1;
        } else {
            verb = Some(t);
            i += 1;
            break;
        }
    }
    let verb = verb.ok_or_else(|| crate::i18n::t("plugin.cmd.no.verb"))?;
    let specs: Vec<String> = tokens[i..].iter().map(|s| s.to_string()).collect();

    match verb.to_ascii_lowercase().as_str() {
        "add" => {
            if specs.is_empty() {
                return Err(crate::i18n::t("plugin.cmd.add.missing.spec"));
            }
            let mut normalized = Vec::new();
            for s in &specs {
                normalized.push(classify_spec(s)?);
            }
            Ok(ParsedAdd { specs: normalized, profile })
        }
        "install" | "i" => Err(crate::i18n::t("plugin.cmd.install.verb")),
        "remove" | "rm" | "uninstall" | "un" => Err(crate::i18n::t("plugin.cmd.remove.verb")),
        "update" | "up" => Err(crate::i18n::t("plugin.cmd.update.verb")),
        "why" | "list" | "ls" => Err(crate::i18n::t("plugin.cmd.query.verb")),
        other => Err(crate::i18n::t_fmt("plugin.cmd.unsupported", &[other])),
    }
}

// ─────────────────────────────── 失败签名识别 ───────────────────────────────

/// 判断输出是否为「dsh 命令无法识别」类错误（PowerShell 中英文 / cmd / shell）。
pub fn is_dsh_missing(output: &str) -> bool {
    let l = output.to_lowercase();
    (output.contains("无法将") && output.contains("识别为") && output.contains("dsh"))
        || l.contains("not recognized as the name of a cmdlet")
        || (l.contains("command not found") && l.contains("dsh"))
        || (l.contains("unknown command") && l.contains("plugin"))
        || (output.contains("dsh") && output.contains("不是内部或外部命令"))
        || l.contains("'dsh' 不是内部或外部命令")
}

/// 判断输出是否为 pnpm 拦截依赖构建脚本（allowBuilds）类错误。
pub fn is_build_blocked(output: &str) -> bool {
    let l = output.to_lowercase();
    l.contains("allowbuilds")
        || l.contains("allow-builds")
        || l.contains("err_pnpm_ignored_builds")
        || l.contains("err_pnpm_git_dep_prepare_not_allowed")
        || l.contains("ignored build")
        || (l.contains("prepare") && l.contains("blocked"))
        || (l.contains("build script") && l.contains("blocked"))
}

// ─────────────────────────────── 构建拦截绕过 ───────────────────────────────

/// 从 pnpm 输出中解析被忽略构建脚本的包名。
/// 格式：`Ignored build scripts: cloudflared@0.7.3, cpu-features@0.0.10, ssh2@1.17.0`
/// （兼容 `Ignored builds:` 变体）；剥离 `@版本`（scoped 包保留前导 `@`）。
pub fn parse_ignored_build_packages(output: &str) -> Vec<String> {
    let mut packages = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        // 真实行形如 `[ERR_PNPM_IGNORED_BUILDS] Ignored build scripts: a@1, b@2`，
        // 关键字位于行中，需 find 而非 strip_prefix。
        let body = if let Some(idx) = trimmed.find("Ignored build scripts:") {
            &trimmed[idx + "Ignored build scripts:".len()..]
        } else if let Some(idx) = trimmed.find("Ignored builds:") {
            &trimmed[idx + "Ignored builds:".len()..]
        } else {
            continue;
        };
        for token in body.split(',') {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            // 剥离 `@版本`：取最后一个 @（scoped 包首字符的 @ 不算版本分隔符）。
            let name = match token.rfind('@') {
                Some(0) => token,
                Some(i) => &token[..i],
                None => token,
            };
            let name = name.trim();
            if !name.is_empty() && !packages.contains(&name.to_string()) {
                packages.push(name.to_string());
            }
        }
    }
    packages
}

/// 在 output 中查找 `marker` 之后第一个引号包裹的片段（marker 以 `"` 结尾，
/// 如 `fetched from "` / `The git-hosted package "`），找不到返回 None。
fn find_quoted<'a>(output: &'a str, marker: &str) -> Option<&'a str> {
    let start = output.find(marker)? + marker.len();
    let rest = &output[start..];
    let end = rest.find('"')?;
    let s = &rest[..end];
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// 向 key 列表追加去重后的条目；跳过通配 `"*"` / `*`（无实际放行作用）。
fn push_allow_key(keys: &mut Vec<String>, k: &str) {
    let k = k.trim();
    if k.is_empty() || k == "*" || k == "\"*\"" || k == "'*'" {
        return;
    }
    if !keys.iter().any(|x| x == k) {
        keys.push(k.to_string());
    }
}

/// 从 pnpm 的 git 托管依赖拦截错误（ERR_PNPM_GIT_DEP_PREPARE_NOT_ALLOWED）中
/// 提取 pnpm 提示要写入 allowBuilds 的**精确 key**，形如
/// `dsh-better-sidebar@https://codeload.github.com/…/tar.gz/<sha>`（含包名与
/// 解析出的 tarball URL，含 commit 哈希；必须原样写入才会被 pnpm 放行）。
///
/// 主解析：示例块（`allowBuilds:` 后缩进的 `key: true` 行，兼容引号形式）；
/// 兜底解析：错误头里的 `fetched from "URL"` + `The git-hosted package "NAME@VERSION"`，
/// 拼成 `NAME@URL`。通配 `"*"` / `*` 无实际作用，跳过；结果去重、保持顺序。
pub fn parse_git_prepare_keys(output: &str) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();

    // 主解析：`allowBuilds:` 示例块（可能有多块，全部收集）。
    let lines: Vec<&str> = output.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim();
        if t == "allowBuilds" || t == "allowBuilds:" || t == "allowBuilds :" {
            i += 1;
            while i < lines.len() {
                let line = lines[i];
                if line.trim().is_empty() || !(line.starts_with(' ') || line.starts_with('\t')) {
                    break;
                }
                let trimmed = line.trim();
                if let Some(key) = trimmed.strip_suffix(": true") {
                    let key = key.trim();
                    // 兼容带引号的 key：`"a@url": true`。
                    let key = if (key.starts_with('"') && key.ends_with('"') && key.len() >= 2)
                        || (key.starts_with('\'') && key.ends_with('\'') && key.len() >= 2)
                    {
                        &key[1..key.len() - 1]
                    } else {
                        key
                    };
                    push_allow_key(&mut keys, key);
                }
                i += 1;
            }
        }
        i += 1;
    }

    // 兜底解析：错误头。
    if keys.is_empty() {
        if let (Some(url), Some(pkg)) =
            (find_quoted(output, "fetched from \""), find_quoted(output, "package \""))
        {
            // pkg 形如 name@version；取最后一个 @ 之前为包名（scoped 包首字符 @ 不算分隔符）。
            let name = match pkg.rfind('@') {
                Some(0) => pkg,
                Some(idx) => &pkg[..idx],
                None => pkg,
            };
            if !name.is_empty() && !url.is_empty() {
                push_allow_key(&mut keys, &format!("{name}@{url}"));
            }
        }
    }

    keys
}

/// 合并解析「被忽略的构建脚本包名」与「git 托管依赖 allowBuilds key」，去重保序。
pub fn parse_blocked_packages(output: &str) -> Vec<String> {
    let mut out = parse_ignored_build_packages(output);
    for k in parse_git_prepare_keys(output) {
        if !out.contains(&k) {
            out.push(k);
        }
    }
    out
}
/// 确保 profile 的 pnpm-workspace.yaml 的 allowBuilds 中，指定包为显式 `true`。
///
/// pnpm（实测 11.x）并不会因为 `allowBuilds: {"*": true}` 通配而放行构建，
/// 必须为被拦截的包写出显式 `包名: true`（与 dsh 自身报错指引一致）。
/// 已存在条目会被整行替换为 `包名: true`（覆盖 `set this to true or false`
/// 之类的占位值），缺失则插入；其余条目原样保留。返回是否发生了修改。
pub fn ensure_allow_builds(dir: &Path, packages: &[&str]) -> Result<bool, String> {
    if packages.is_empty() {
        return Ok(false);
    }
    let path = dir.join("pnpm-workspace.yaml");
    let existing = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => String::new(),
    };

    let mut changed = false;
    let mut out: Vec<String> = if existing.is_empty() {
        Vec::new()
    } else {
        existing.lines().map(|l| l.to_string()).collect()
    };

    // 查找 allowBuilds: 段起始行。
    let mut section: Option<usize> = None;
    for (i, line) in out.iter().enumerate() {
        let t = line.trim_start();
        if t.starts_with("allowBuilds:") || t.starts_with("allowBuilds :") {
            section = Some(i);
            break;
        }
    }

    match section {
        None => {
            // 无 allowBuilds 段：追加新段。
            if !out.is_empty() && !out.last().map(|l| l.is_empty()).unwrap_or(false) {
                out.push(String::new());
            }
            out.push("allowBuilds:".to_string());
            for p in packages {
                out.push(format!("  {p}: true"));
            }
            changed = true;
        }
        Some(idx) => {
            // 段内条目：allowBuilds: 之后缩进的行。
            let entry_start = idx + 1;
            let mut entry_end = entry_start;
            while entry_end < out.len() {
                let line = &out[entry_end];
                if line.is_empty() || !(line.starts_with(' ') || line.starts_with('\t')) {
                    break;
                }
                entry_end += 1;
            }
            for p in packages {
                // 已存在该包条目 → 整行替换为 `  <pkg>: true`。
                let mut found = false;
                for i in entry_start..entry_end {
                    let t = out[i].trim_start();
                    if t.starts_with(p) && t[p.len()..].starts_with(':') {
                        let indent: String = out[i].chars().take_while(|c| *c == ' ').collect();
                        let new_line = format!("{indent}{p}: true");
                        if out[i] != new_line {
                            out[i] = new_line;
                            changed = true;
                        }
                        found = true;
                        break;
                    }
                }
                if !found {
                    out.insert(entry_start, format!("  {p}: true"));
                    entry_end += 1;
                    changed = true;
                }
            }
        }
    }

    if !changed {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败：{e}"))?;
    }
    let content = out.join("\n") + "\n";
    std::fs::write(&path, content).map_err(|e| format!("写入 pnpm-workspace.yaml 失败：{e}"))?;
    Ok(true)
}

// ─────────────────────────────── 探测与执行 ───────────────────────────────

/// 安装命令参数：`dsh plugin … add <spec...>`。
/// 缺少 add 动词时，pnpm 会把包名当作子命令执行而报
/// `ERR_PNPM_RECURSIVE_EXEC_FIRST_FAIL Command "@..." not found`。
pub fn install_args(specs: &[String]) -> Vec<String> {
    let mut args = vec!["add".to_string()];
    args.extend(specs.iter().cloned());
    args
}

/// 探测全局 dsh 是否可识别 plugin 子命令（`dsh plugin --help` 退出码 0）。
pub fn probe_global_dsh_plugin() -> bool {
    let mut child = match spawn_any(
        &["dsh.cmd", "dsh"],
        &["plugin".into(), "--help".into()],
        None,
        &[],
    ) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(_) => return false,
        }
    }
}

/// 单次命令执行结果（供修复安装等模块复用）。
pub(crate) struct RunOutcome {
    pub(crate) ok: bool,
    pub(crate) exit_code: i32,
    pub(crate) output: String,
}

/// 执行单个命令：输出行流式转发到 channel，并保留最近若干行用于失败分析。
pub(crate) fn run_capture(
    programs: &[&str],
    argv: &[String],
    cwd: &Path,
    envs: &[(&str, String)],
    step_id: &str,
    channel: &Channel<PipelineEvent>,
) -> Result<RunOutcome, String> {
    let mut child = spawn_any(programs, argv, Some(&cwd.to_path_buf()), envs).map_err(|e| e.friendly())?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let kept: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let ch = channel.clone();
    let sid = step_id.to_string();
    let kept1 = kept.clone();
    if let Some(out) = stdout {
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let _ = ch.send(PipelineEvent::Output {
                    step: sid.clone(),
                    stream: "stdout".into(),
                    line: line.clone(),
                });
                let mut v = kept1.lock().unwrap();
                v.push(line);
                if v.len() > MAX_KEPT_LINES {
                    v.remove(0);
                }
            }
        });
    }
    let ch = channel.clone();
    let sid = step_id.to_string();
    let kept2 = kept.clone();
    if let Some(err) = stderr {
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                let _ = ch.send(PipelineEvent::Output {
                    step: sid.clone(),
                    stream: "stderr".into(),
                    line: line.clone(),
                });
                let mut v = kept2.lock().unwrap();
                v.push(line);
                if v.len() > MAX_KEPT_LINES {
                    v.remove(0);
                }
            }
        });
    }

    let status = child.wait().map_err(|e| AppError::Io(e.to_string()).friendly())?;
    // 等待输出线程把尾部行写入缓冲区（子进程已退出，管道即将 EOF）。
    std::thread::sleep(std::time::Duration::from_millis(150));
    let output = kept.lock().unwrap().join("\n");
    Ok(RunOutcome {
        ok: status.success(),
        exit_code: status.code().unwrap_or(-1),
        output,
    })
}

/// 构建某安装方式的 `dsh plugin` 命令（返回 (程序名列表, argv 前缀)）。
/// - prebuilt：安装目录下的 `node_modules\.bin\dsh.cmd`；
/// - source + usePnpmDsh：`pnpm dsh plugin …`；
/// - source + 非 usePnpmDsh：`dsh plugin …`。
fn dsh_plugin_invocation(
    install_dir: &Path,
    profile: &str,
    args: &[String],
    use_pnpm: bool,
    mode: &str,
) -> (Vec<String>, Vec<String>) {
    if mode == "prebuilt" {
        let dsh = install_dir.join("node_modules").join(".bin").join("dsh.cmd");
        let mut argv = vec!["plugin".into(), "--profile".into(), profile.to_string()];
        argv.extend(args.iter().cloned());
        (vec![dsh.to_string_lossy().to_string()], argv)
    } else if use_pnpm {
        let mut argv = vec![
            "dsh".into(),
            "plugin".into(),
            "--profile".into(),
            profile.to_string(),
        ];
        argv.extend(args.iter().cloned());
        (vec!["pnpm.cmd".into(), "pnpm".into()], argv)
    } else {
        let mut argv = vec![
            "plugin".into(),
            "--profile".into(),
            profile.to_string(),
        ];
        argv.extend(args.iter().cloned());
        (vec!["dsh.cmd".into(), "dsh".into()], argv)
    }
}

/// 执行一次 `dsh plugin --profile <profile> <args>` 操作（按安装方式分派命令）。
///
/// 失败自动重试（最多各一次）：
/// - 全局 `dsh` 不可识别 → 切换为 `pnpm dsh`（写回配置；仅源码模式）；
/// - pnpm 拦截构建脚本 → 为 profile 配置 allowBuilds 并重试。
pub fn run_plugin_op(
    app: &AppHandle,
    profile: &str,
    args: &[String],
    action_key: &str,
    subject: &str,
    channel: &Channel<PipelineEvent>,
    logger: &Logger,
) -> Result<PluginOpResult, String> {
    let action_label = crate::i18n::t(action_key);
    let cfg: AppConfig = config::load_config(app);
    let mode = cfg.install_mode.clone();
    let install_dir = cfg
        .install_dir
        .clone()
        .ok_or_else(|| AppError::NotInstalled.friendly())?;
    let cwd = PathBuf::from(&install_dir);
    if mode == "source" {
        if !crate::detect::is_valid_repo(&cwd) {
            return Err(AppError::NotInstalled.friendly());
        }
    } else if !crate::detect::is_valid_prebuilt(&cwd) {
        return Err(AppError::NotInstalled.friendly());
    }
    let pdir = profile_dir(profile)?;

    let display = if mode == "prebuilt" {
        format!("node_modules\\.bin\\dsh.cmd plugin --profile {profile} {}", args.join(" "))
    } else {
        format!("dsh plugin --profile {profile} {}", args.join(" "))
    };
    logger.info(&crate::i18n::t_fmt(
        "log.plugin_op_start",
        &[&action_label, &display, &install_dir],
    ));

    let mut use_pnpm = cfg.use_pnpm_dsh;
    let mut bypass_builds: u8 = 0;
    let mut attempt = 0;
    loop {
        attempt += 1;
        let step_id = format!("plugin-{attempt}");
        let title = format!("{action_label} ({display})");
        let _ = channel.send(PipelineEvent::StepStarted {
            id: step_id.clone(),
            title: title.clone(),
        });

        let (programs_vec, argv) =
            dsh_plugin_invocation(&cwd, profile, args, use_pnpm, &mode);
        let program_refs: Vec<&str> = programs_vec.iter().map(|s| s.as_str()).collect();

        let outcome = match run_capture(&program_refs, &argv, &cwd, &[], &step_id, channel) {
            Ok(o) => o,
            Err(e) => {
                let _ = channel.send(PipelineEvent::Error { message: e.clone() });
                logger.error(&e);
                return Err(e);
            }
        };
        let _ = channel.send(PipelineEvent::StepFinished {
            id: step_id,
            exit_code: outcome.exit_code,
        });

        if outcome.ok {
            let msg = crate::i18n::t_fmt("plugin.op.ok", &[&action_label, subject]);
            logger.info(&msg);
            let _ = channel.send(PipelineEvent::Finished { ok: true });
            return Ok(PluginOpResult {
                ok: true,
                message: msg,
                action: args.first().cloned().unwrap_or_default(),
            });
        }

        // 失败 1：全局 dsh 不可识别 → 切换 pnpm dsh（仅源码模式；预构建模式用本地 dsh.cmd）。
        if !use_pnpm && mode != "prebuilt" && is_dsh_missing(&outcome.output) {
            use_pnpm = true;
            let mut cfg2 = config::load_config(app);
            cfg2.use_pnpm_dsh = true;
            if let Err(e) = config::save_config(app, &cfg2) {
                logger.warn(&crate::i18n::t_fmt("plugin.op.dsh.switch.savefail", &[&e]));
            }
            let note = crate::i18n::t("plugin.op.dsh.switch");
            logger.warn(&note);
            let _ = channel.send(PipelineEvent::Output {
                step: "panel".into(),
                stream: "stderr".into(),
                line: note.into(),
            });
            continue;
        }
        // 失败 2：pnpm 拦截构建脚本 → 把被拦截的包显式加入 allowBuilds 后重试。
        // （实测 pnpm 不会因 `"*": true` 通配放行，必须显式 `包名: true`；
        // git 托管插件报 ERR_PNPM_GIT_DEP_PREPARE_NOT_ALLOWED，key 含解析出的
        // tarball URL，需原样写入。最多自动放行两次，避免死循环。）
        if bypass_builds < 2 && is_build_blocked(&outcome.output) {
            bypass_builds += 1;
            let packages = parse_blocked_packages(&outcome.output);
            if packages.is_empty() {
                logger.warn(&crate::i18n::t("plugin.op.builds.parse"));
            } else {
                let changed = match ensure_allow_builds(&pdir, &packages.iter().map(String::as_str).collect::<Vec<_>>()) {
                    Ok(c) => c,
                    Err(e) => {
                        logger.warn(&crate::i18n::t_fmt("plugin.op.builds.writefail", &[&e]));
                        false
                    }
                };
                let joined = packages.join("、");
                let note = if changed {
                    crate::i18n::t_fmt("plugin.op.builds.note1", &[&joined])
                } else {
                    crate::i18n::t_fmt("plugin.op.builds.note2", &[&joined])
                };
                logger.warn(&note);
                let _ = channel.send(PipelineEvent::Output {
                    step: "panel".into(),
                    stream: "stderr".into(),
                    line: note.clone(),
                });
                continue;
            }
        }

        // 最终失败：摘录最有信息量的输出行作为友好提示（优先 pnpm 的 ERR 行，
        // 避免摘到无意义的 `[ELIFECYCLE] Command failed with exit code 1.`）。
        let no_output = crate::i18n::t("plugin.op.no.output");
        let pick = outcome
            .output
            .lines()
            .filter(|l| !l.trim().is_empty())
            .rev()
            .find(|l| {
                let t = l.trim();
                t.starts_with("[ERR_PNPM") || t.contains("ERR_PNPM_")
            })
            .or_else(|| {
                outcome
                    .output
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
            });
        let last = pick.unwrap_or(&no_output).trim();
        let last = if last.chars().count() > 200 {
            let s: String = last.chars().take(200).collect();
            format!("{s}…")
        } else {
            last.to_string()
        };
        let msg = crate::i18n::t_fmt(
            "plugin.op.failed",
            &[&action_label, &outcome.exit_code.to_string(), &last],
        );
        logger.error(&msg);
        let _ = channel.send(PipelineEvent::Error { message: msg.clone() });
        return Err(msg);
    }
}

// ─────────────────────────────── 插件更新检测 ───────────────────────────────

/// 单个插件的更新检测结果。
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PluginUpdateInfo {
    /// 依赖 key（与插件列表一致）。
    pub key: String,
    /// 当前已安装版本（node_modules 的 package.json；未知为 None）。
    pub current_version: Option<String>,
    /// 最新可用版本（无法获取为 None）。
    pub latest_version: Option<String>,
    /// 检测到有更新（latest > current）。
    pub update_available: bool,
    /// 来源：npm / github / unknown。
    pub source: String,
}

/// 指定 profile 的插件更新检测结果。
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PluginUpdates {
    pub profile: String,
    pub checked_at: String,
    pub entries: Vec<PluginUpdateInfo>,
}

/// 运行命令并捕获 stdout+stderr（禁终端窗口；带超时，超时/失败返回 None）。
fn run_query(programs: &[&str], args: &[String], cwd: &Path, timeout: Duration) -> Option<String> {
    let mut last_err: Option<std::io::Error> = None;
    for prog in programs {
        let mut cmd = Command::new(prog);
        no_window(&mut cmd);
        cmd.args(args).current_dir(cwd);
        cmd.env("PATH", crate::config::augmented_path());
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        match cmd.spawn() {
            Ok(mut child) => {
                let stdout = child.stdout.take();
                let out = Arc::new(Mutex::new(String::new()));
                if let Some(mut o) = stdout {
                    let o1 = out.clone();
                    std::thread::spawn(move || {
                        let mut s = String::new();
                        let _ = o.read_to_string(&mut s);
                        *o1.lock().unwrap() += &s;
                    });
                }
                if let Some(mut e) = child.stderr.take() {
                    let o2 = out.clone();
                    std::thread::spawn(move || {
                        let mut s = String::new();
                        let _ = e.read_to_string(&mut s);
                        *o2.lock().unwrap() += &s;
                    });
                }
                let deadline = Instant::now() + timeout;
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) => break,
                        Ok(None) => {
                            if Instant::now() > deadline {
                                let _ = child.kill();
                                let _ = child.wait();
                                return None;
                            }
                            std::thread::sleep(Duration::from_millis(60));
                        }
                        Err(_) => return None,
                    }
                }
                // 等待读取线程把输出写完。
                std::thread::sleep(Duration::from_millis(50));
                let res = out.lock().unwrap().trim().to_string();
                if res.is_empty() {
                    return None;
                }
                return Some(res);
            }
            Err(e) => last_err = Some(e),
        }
    }
    let _ = last_err;
    None
}

/// 是否为 npm 依赖（版本范围 / 精确版 / tag 等），非 git/文件/workspace 链接且不含 `#`。
pub fn is_npm_spec(spec: &str) -> bool {
    let s = spec.trim();
    if s.is_empty() {
        return false;
    }
    let lower = s.to_ascii_lowercase();
    for prefix in [
        "github:", "git+", "git@", "ssh://", "file:", "link:", "workspace:", "portal:", "catalog:", "http://", "https://",
    ] {
        if lower.starts_with(prefix) {
            return false;
        }
    }
    !s.contains('#')
}

/// 从插件 spec 解析 GitHub 仓库标识 `owner/repo`（多形态；非 github 返回 None）。
pub fn github_repo_from_spec(spec: &str) -> Option<String> {
    let s = spec.trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    // 纯 `owner/repo` GitHub 简写：无协议/冒号/@/路径穿越，恰两段合法字符。
    let bare_ok = !s.contains("://")
        && !s.contains('@')
        && !s.contains(':')
        && s.split('/').count() == 2
        && s.split('/').all(|seg| {
            !seg.is_empty()
                && seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        });
    if !(lower.contains("github.com")
        || s.starts_with("github:")
        || s.starts_with("git@github.com:")
        || s.starts_with("git+https://github.com/")
        || bare_ok)
    {
        return None;
    }
    // 去掉 github: 前缀与 #ref。
    let s = if let Some(rest) = s.strip_prefix("github:") {
        rest
    } else {
        s
    };
    let s = s.split('#').next().unwrap_or(s).trim();
    // 依次剥离协议前缀 / git+ / git@ssh 形式 / 域名。
    let mut p = s;
    if let Some(r) = p.strip_prefix("git+") {
        p = r;
    }
    if let Some(r) = p.strip_prefix("https://") {
        p = r;
    } else if let Some(r) = p.strip_prefix("http://") {
        p = r;
    }
    if let Some(r) = p.strip_prefix("git@github.com:") {
        p = r;
    }
    if let Some(r) = p.strip_prefix("github.com/") {
        p = r;
    }
    let p = p.trim().trim_end_matches(".git").trim_matches('/');
    let mut segs = p.split('/');
    let owner = segs.next()?;
    let repo = segs.next()?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

/// 从 `git ls-remote --tags` 输出中选取最高的语义化版本 tag。
pub fn pick_latest_tag(output: &str) -> Option<String> {
    fn parse_ver(s: &str) -> Option<semver::Version> {
        let t = s.trim().trim_start_matches('v');
        semver::Version::parse(t).ok()
    }
    let mut best: Option<(semver::Version, String)> = None;
    for line in output.lines() {
        let token = line.split_whitespace().last().unwrap_or("");
        let tag = token.strip_prefix("refs/tags/").unwrap_or("");
        let tag = tag.trim_end_matches("^{}");
        if tag.is_empty() {
            continue;
        }
        if let Some(v) = parse_ver(tag) {
            if best.as_ref().map(|(bv, _)| v > *bv).unwrap_or(true) {
                best = Some((v, tag.to_string()));
            }
        }
    }
    best.map(|(_, t)| t)
}

/// semver 比较：a < b 且两者均可解析时返回 true。
fn version_less(a: &str, b: &str) -> bool {
    let pa = semver::Version::parse(a.trim().trim_start_matches('v')).ok();
    let pb = semver::Version::parse(b.trim().trim_start_matches('v')).ok();
    match (pa, pb) {
        (Some(x), Some(y)) => x < y,
        _ => false,
    }
}

/// 查询 npm 包的最新版本（`<pkg> view <name> version`），失败返回 None。
fn query_latest_npm(pkg: &str, cwd: &Path) -> Option<String> {
    let args = vec!["view".to_string(), pkg.to_string(), "version".to_string()];
    let out = run_query(&["pnpm.cmd", "pnpm", "npm.cmd", "npm"], &args, cwd, Duration::from_secs(20))?;
    out.lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty() && !l.to_ascii_lowercase().contains("notice"))
        .map(String::from)
}

/// 查询 GitHub 仓库的最高版本 tag，失败返回 None。
fn query_latest_github_tag(repo: &str, cwd: &Path) -> Option<String> {
    let url = format!("https://github.com/{repo}");
    let args = vec![
        "ls-remote".to_string(),
        "--tags".to_string(),
        "--refs".to_string(),
        url,
    ];
    let out = run_query(&["git.cmd", "git"], &args, cwd, Duration::from_secs(20))?;
    pick_latest_tag(&out)
}

fn make_update_info(key: String, current: Option<String>, latest: Option<String>, source: &str) -> PluginUpdateInfo {
    let update_available = match (&current, &latest) {
        (Some(c), Some(l)) => version_less(c, l),
        _ => false,
    };
    PluginUpdateInfo {
        key,
        current_version: current,
        latest_version: latest,
        update_available,
        source: source.to_string(),
    }
}

/// 检测指定 profile 已安装第三方插件的更新。
/// npm 类查询 registry 最新版；github 类查询远程最高版本 tag；其余标记 unknown。
/// 网络失败 / 超时 / 包不存在：该项 latest 为 None、不标记更新，不中断整体。
pub fn check_plugin_updates(profile: &str, install_dir: &str) -> Result<PluginUpdates, String> {
    let pdir = profile_dir(profile)?;
    let cwd = Path::new(install_dir);
    let list = read_plugin_list(&pdir, profile, true);
    let mut entries = Vec::new();
    for e in &list.entries {
        if is_npm_spec(&e.spec) {
            let latest = query_latest_npm(&e.key, cwd);
            entries.push(make_update_info(e.key.clone(), e.version.clone(), latest, "npm"));
        } else if let Some(repo) = github_repo_from_spec(&e.spec) {
            let latest = query_latest_github_tag(&repo, cwd);
            entries.push(make_update_info(e.key.clone(), e.version.clone(), latest, "github"));
        } else {
            entries.push(make_update_info(e.key.clone(), e.version.clone(), None, "unknown"));
        }
    }
    Ok(PluginUpdates {
        profile: profile.to_string(),
        checked_at: crate::config::now_string(),
        entries,
    })
}

// ─────────────────────────────── 测试 ───────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("dsh-plugin-{name}-{}", std::process::id()))
    }

    // ---------- profile_dir ----------

    #[test]
    fn profile_dir_resolves_under_dsh_home() {
        let expected = PathBuf::from(crate::detect::dsh_home())
            .join("profiles")
            .join("web");
        assert_eq!(profile_dir("web").unwrap(), expected);
    }

    #[test]
    fn profile_dir_rejects_invalid_names() {
        for bad in ["", ".", "..", "node_modules", "a/b", "a\\b"] {
            assert!(profile_dir(bad).is_err(), "should reject {bad:?}");
        }
        // 首尾空白会被裁剪后接受。
        assert_eq!(profile_dir(" web ").unwrap(), profile_dir("web").unwrap());
    }

    // ---------- read_plugin_list ----------

    #[test]
    fn read_plugin_list_parses_manifest() {
        let dir = tmp_dir("list1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("package.json"),
            r#"{
  "name": "dsh-profile-web",
  "private": true,
  "dependencies": {
    "@linxin666/dsh-web-ui-all": "^1.2.0",
    "plain-lib": "1.0.0",
    "github.com/some/plugin": "github.com/some/plugin#dev"
  },
  "dsh": { "profile": { "bundles": ["@deepseek-ai/dsh-base", "@linxin666/dsh-web-ui-all"] } }
}"#,
        )
        .unwrap();
        let list = read_plugin_list(&dir, "web", true);
        assert!(list.initialized);
        assert_eq!(list.entries.len(), 3);
        let ui = list.entries.iter().find(|e| e.key == "@linxin666/dsh-web-ui-all").unwrap();
        assert!(ui.is_bundle);
        assert_eq!(ui.spec, "^1.2.0");
        let plain = list.entries.iter().find(|e| e.key == "plain-lib").unwrap();
        assert!(!plain.is_bundle);
        // 内置组合包：在 bundles 中但不在 dependencies。
        assert_eq!(list.builtin_bundles, vec!["@deepseek-ai/dsh-base"]);
        assert_eq!(list.use_pnpm_dsh, true);
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---------- 已安装版本 ----------

    #[test]
    fn installed_version_reads_package_json() {
        let dir = tmp_dir("ver1");
        // 普通包 + scoped 包。
        std::fs::create_dir_all(dir.join("node_modules/dsh-better-sidebar")).unwrap();
        std::fs::create_dir_all(dir.join("node_modules/@scope/name")).unwrap();
        std::fs::write(
            dir.join("node_modules/dsh-better-sidebar/package.json"),
            r#"{"name":"dsh-better-sidebar","version":"0.13.1"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("node_modules/@scope/name/package.json"),
            r#"{"name":"@scope/name","version":"2.3.4"}"#,
        )
        .unwrap();
        assert_eq!(
            installed_version(&dir, "dsh-better-sidebar").as_deref(),
            Some("0.13.1")
        );
        assert_eq!(installed_version(&dir, "@scope/name").as_deref(), Some("2.3.4"));
        // 未安装 / 缺失 / 解析失败 → None。
        assert_eq!(installed_version(&dir, "not-installed"), None);
        std::fs::write(
            dir.join("node_modules/@scope/name/package.json"),
            "not json",
        )
        .unwrap();
        assert_eq!(installed_version(&dir, "@scope/name"), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_plugin_list_fills_installed_version() {
        let dir = tmp_dir("ver2");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(dir.join("node_modules/github-plugin")).unwrap();
        std::fs::write(
            dir.join("node_modules/github-plugin/package.json"),
            r#"{"name":"github-plugin","version":"0.13.1"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("package.json"),
            r#"{
  "name": "dsh-profile-web",
  "private": true,
  "dependencies": {
    "github-plugin": "github:omdsh-dev/dsh-better-sidebar",
    "plain-lib": "1.0.0"
  }
}"#,
        )
        .unwrap();
        let list = read_plugin_list(&dir, "web", true);
        let gp = list.entries.iter().find(|e| e.key == "github-plugin").unwrap();
        assert_eq!(gp.spec, "github:omdsh-dev/dsh-better-sidebar");
        assert_eq!(gp.version.as_deref(), Some("0.13.1"));
        // 未安装的依赖：version 为 None（前端回退显示 spec）。
        let plain = list.entries.iter().find(|e| e.key == "plain-lib").unwrap();
        assert_eq!(plain.version, None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_plugin_list_handles_missing_manifest() {
        let dir = tmp_dir("list2");
        std::fs::create_dir_all(&dir).unwrap();
        let list = read_plugin_list(&dir, "web", false);
        assert!(!list.initialized);
        assert!(list.entries.is_empty());
        assert!(list.builtin_bundles.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---------- parse_add_input ----------

    #[test]
    fn parse_bare_npm_package() {
        let p = parse_add_input("lodash").unwrap();
        assert_eq!(p.specs, vec!["lodash"]);
        assert_eq!(p.profile, None);
        let p = parse_add_input("lodash@4.17.21").unwrap();
        assert_eq!(p.specs, vec!["lodash@4.17.21"]);
        let p = parse_add_input(" @linxin666/dsh-web-ui-all ").unwrap();
        assert_eq!(p.specs, vec!["@linxin666/dsh-web-ui-all"]);
        let p = parse_add_input("@scope/pkg@^1.2.0").unwrap();
        assert_eq!(p.specs, vec!["@scope/pkg@^1.2.0"]);
    }

    #[test]
    fn parse_github_forms() {
        assert_eq!(
            parse_add_input("github:linxin666/dsh-web-ui-all").unwrap().specs,
            vec!["github:linxin666/dsh-web-ui-all"]
        );
        assert_eq!(
            parse_add_input("github:linxin666/dsh-web-ui-all#v1.0.0").unwrap().specs,
            vec!["github:linxin666/dsh-web-ui-all#v1.0.0"]
        );
        assert_eq!(
            parse_add_input("https://github.com/linxin666/dsh-web-ui-all").unwrap().specs,
            vec!["github:linxin666/dsh-web-ui-all"]
        );
        assert_eq!(
            parse_add_input("https://github.com/linxin666/dsh-web-ui-all#dev").unwrap().specs,
            vec!["github:linxin666/dsh-web-ui-all#dev"]
        );
        assert_eq!(
            parse_add_input("https://github.com/linxin666/dsh-web-ui-all.git").unwrap().specs,
            vec!["github:linxin666/dsh-web-ui-all"]
        );
        assert_eq!(
            parse_add_input("http://github.com/a/b").unwrap().specs,
            vec!["github:a/b"]
        );
        // owner/repo GitHub 简写（pnpm 原生支持）。
        assert_eq!(
            parse_add_input("linxin666/dsh-web-ui-all").unwrap().specs,
            vec!["linxin666/dsh-web-ui-all"]
        );
        // git+ / git@ 透传。
        assert_eq!(
            parse_add_input("git+https://github.com/a/b.git").unwrap().specs,
            vec!["git+https://github.com/a/b.git"]
        );
    }

    #[test]
    fn parse_github_tarball_urls_pass_through() {
        // 插件官方文档常见的 GitHub 归档压缩包链接：原样透传（pnpm 按远程 tarball 安装）。
        let url = "https://github.com/omdsh-dev/dsh-at-file/archive/refs/tags/v0.6.3.tar.gz";
        assert_eq!(parse_add_input(url).unwrap().specs, vec![url]);
        // 完整命令形式：spec 原样，--profile 照常识别。
        let p = parse_add_input(&format!("dsh plugin --profile web add {url}")).unwrap();
        assert_eq!(p.specs, vec![url]);
        assert_eq!(p.profile.as_deref(), Some("web"));
        // GitHub release 附件 .tgz：透传。
        let rel = "https://github.com/a/b/releases/download/v1.0.0/pkg.tgz";
        assert_eq!(parse_add_input(rel).unwrap().specs, vec![rel]);
        // /archive/ 路径（含 .zip）：解析层透传，交给 pnpm 判断。
        let zip = "https://github.com/a/b/archive/refs/tags/v1.0.0.zip";
        assert_eq!(parse_add_input(zip).unwrap().specs, vec![zip]);
        // 非压缩包的仓库内路径仍报错（既有行为不变）。
        assert!(parse_add_input("https://github.com/owner/repo/tree/main").is_err());
    }

    // ---------- list_profiles ----------

    #[test]
    fn list_profiles_reads_subdirs_only() {
        let base = std::env::temp_dir().join(format!("dsh-profiles-{}", std::process::id()));
        let dir = base.join("profiles");
        std::fs::create_dir_all(dir.join("web")).unwrap();
        std::fs::create_dir_all(dir.join("headless")).unwrap();
        std::fs::write(dir.join("web/package.json"), "{}").unwrap();
        // 普通文件不是 profile。
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        assert_eq!(list_profiles_in(&dir), vec!["headless", "web"]);
        // 目录不存在 → 空。
        assert!(list_profiles_in(&base.join("nope")).is_empty());
        std::fs::remove_dir_all(&base).ok();
    }

    // ---------- 插件更新检测 ----------

    #[test]
    fn npm_spec_detection() {
        assert!(is_npm_spec("^1.2.0"));
        assert!(is_npm_spec("~2.0.0"));
        assert!(is_npm_spec("1.0.0"));
        assert!(is_npm_spec("latest"));
        assert!(is_npm_spec("@scope/pkg"));
        // 非 npm。
        assert!(!is_npm_spec("github:omdsh-dev/dsh-better-sidebar"));
        assert!(!is_npm_spec("git+https://github.com/a/b.git"));
        assert!(!is_npm_spec("file:../x"));
        assert!(!is_npm_spec("link:./y"));
        assert!(!is_npm_spec("workspace:*"));
        assert!(!is_npm_spec("https://github.com/a/b"));
        assert!(!is_npm_spec("github:owner/repo#tag"));
        assert!(!is_npm_spec(""));
    }

    #[test]
    fn github_repo_extraction() {
        assert_eq!(
            github_repo_from_spec("github:omdsh-dev/dsh-better-sidebar").as_deref(),
            Some("omdsh-dev/dsh-better-sidebar")
        );
        assert_eq!(
            github_repo_from_spec("github:omdsh-dev/dsh-better-sidebar#v0.13.1").as_deref(),
            Some("omdsh-dev/dsh-better-sidebar")
        );
        assert_eq!(
            github_repo_from_spec("https://github.com/omdsh-dev/dsh-at-file/archive/refs/tags/v0.6.3.tar.gz").as_deref(),
            Some("omdsh-dev/dsh-at-file")
        );
        assert_eq!(
            github_repo_from_spec("git+https://github.com/a/b.git").as_deref(),
            Some("a/b")
        );
        assert_eq!(github_repo_from_spec("omdsh-dev/dsh-better-sidebar").as_deref(), Some("omdsh-dev/dsh-better-sidebar"));
        // 非 github：不误判。
        assert_eq!(github_repo_from_spec("https://gitlab.com/a/b"), None);
        assert_eq!(github_repo_from_spec("file:../x"), None);
        assert_eq!(github_repo_from_spec(""), None);
        assert_eq!(github_repo_from_spec("github:"), None);
    }

    #[test]
    fn latest_tag_picks_highest_semver() {
        let out = "aaa\trefs/tags/v0.10.0\nbbb\trefs/tags/v0.9.1\nccc\trefs/tags/v0.10.0^{}\nddd\trefs/tags/release\n";
        assert_eq!(pick_latest_tag(out).as_deref(), Some("v0.10.0"));
        let out2 = "aaa\trefs/tags/1.2.3\nbbb\trefs/tags/2.0.0\n";
        assert_eq!(pick_latest_tag(out2).as_deref(), Some("2.0.0"));
        assert_eq!(pick_latest_tag("aaa\trefs/tags/release"), None);
        assert_eq!(pick_latest_tag(""), None);
    }

    #[test]
    fn version_less_is_strict() {
        assert!(version_less("0.13.0", "0.13.1"));
        assert!(version_less("1.0.0", "1.2.3"));
        assert!(!version_less("1.2.3", "1.2.3"));
        assert!(!version_less("1.3.0", "1.2.3"));
        assert!(!version_less("abc", "1.0.0"));
        assert!(!version_less("1.0.0", "abc"));
    }

    #[test]
    fn parse_full_commands() {
        let p = parse_add_input("dsh plugin --profile web add @linxin666/dsh-web-ui-all").unwrap();
        assert_eq!(p.specs, vec!["@linxin666/dsh-web-ui-all"]);
        assert_eq!(p.profile.as_deref(), Some("web"));

        let p = parse_add_input("pnpm dsh plugin --profile headless add github:a/b#dev").unwrap();
        assert_eq!(p.specs, vec!["github:a/b#dev"]);
        assert_eq!(p.profile.as_deref(), Some("headless"));

        let p = parse_add_input("pnpm exec dsh plugin add lodash").unwrap();
        assert_eq!(p.specs, vec!["lodash"]);
        assert_eq!(p.profile, None);

        let p = parse_add_input("pnpm.cmd dsh plugin --profile=web add @x/y").unwrap();
        assert_eq!(p.specs, vec!["@x/y"]);
        assert_eq!(p.profile.as_deref(), Some("web"));

        // 带 PowerShell 环境变量前缀。
        let p = parse_add_input("$env:PNPM_ALLOW_BUILDS=\"*\"; pnpm dsh plugin --profile web update @x/y");
        assert!(p.is_err(), "update 应被拒绝");
        let p = parse_add_input("$env:PNPM_ALLOW_BUILDS=\"*\"; dsh plugin --profile web add @x/y").unwrap();
        assert_eq!(p.specs, vec!["@x/y"]);

        // 多包安装。
        let p = parse_add_input("dsh plugin add a b").unwrap();
        assert_eq!(p.specs, vec!["a", "b"]);

        // pnpm add 直接形式（仍走 dsh plugin 通道）。
        let p = parse_add_input("pnpm add github:a/b").unwrap();
        assert_eq!(p.specs, vec!["github:a/b"]);
    }

    #[test]
    fn parse_rejects_wrong_verbs() {
        assert!(parse_add_input("dsh plugin --profile web remove @x/y").is_err());
        assert!(parse_add_input("dsh plugin --profile web update github:a/b").is_err());
        assert!(parse_add_input("pnpm dsh plugin rm @x/y").is_err());
        assert!(parse_add_input("dsh plugin --profile web list").is_err());
        assert!(parse_add_input("dsh plugin --profile web install").is_err());
        assert!(parse_add_input("dsh plugin --profile web why @x/y").is_err());
    }

    #[test]
    fn parse_rejects_bad_input() {
        assert!(parse_add_input("").is_err());
        assert!(parse_add_input("   ").is_err());
        assert!(parse_add_input("not a spec").is_err());
        assert!(parse_add_input("github:").is_err());
        assert!(parse_add_input("github:owner").is_err());
        assert!(parse_add_input("https://github.com/owner").is_err());
        assert!(parse_add_input("https://github.com/owner/repo/tree/main").is_err());
        assert!(parse_add_input("@scope").is_err());
        assert!(parse_add_input("github:owner/repo#").is_err());
        assert!(parse_add_input("dsh plugin add").is_err());
        assert!(parse_add_input("dsh plugin --profile").is_err());
    }

    // ---------- 失败签名 ----------

    #[test]
    fn dsh_missing_signatures() {
        let zh = "dsh : 无法将“dsh”项识别为 cmdlet、函数、脚本文件或可运行程序的名称。请检查名称的拼写，如果包括路径，请确保路径正确，然后再试一次。";
        assert!(is_dsh_missing(zh));
        let en = "dsh: The term 'dsh' is not recognized as the name of a cmdlet, function, script file, or operable program.";
        assert!(is_dsh_missing(en));
        assert!(is_dsh_missing("sh: dsh: command not found"));
        assert!(is_dsh_missing("error: unknown command 'plugin'"));
        assert!(is_dsh_missing("'dsh' 不是内部或外部命令，也不是可运行的程序"));
        assert!(!is_dsh_missing("pnpm error: something else"));
        assert!(!is_dsh_missing(""));
    }

    #[test]
    fn build_blocked_signatures() {
        assert!(is_build_blocked("Ignored build scripts: foo. Run \"pnpm approve-builds\""));
        assert!(is_build_blocked("ERR_PNPM_IGNORED_BUILDS Ignored builds: x"));
        assert!(is_build_blocked("add the exact key pnpm printed above under allowBuilds"));
        assert!(is_build_blocked("build script of \"x\" is blocked until allowed"));
        assert!(is_build_blocked("prepare script of github:... was blocked"));
        assert!(is_build_blocked("[ERR_PNPM_GIT_DEP_PREPARE_NOT_ALLOWED] ..."));
        assert!(!is_build_blocked("pnpm install completed"));
        assert!(!is_build_blocked(""));
    }

    // ---------- git 托管插件 allowBuilds key 解析 ----------

    #[test]
    fn git_prepare_keys_extracts_example_block() {
        // 用户实际日志：pnpm 提示按示例块原样写入 allowBuilds。
        let out = "[ERR_PNPM_GIT_DEP_PREPARE_NOT_ALLOWED] Failed to prepare git-hosted package fetched from \"https://codeload.github.com/omdsh-dev/dsh-better-sidebar/tar.gz/b5e8665499116778fcdd0bbad2f12d7df28664d3\": The git-hosted package \"dsh-better-sidebar@0.13.0\" needs to execute build scripts but is not in the \"allowBuilds\" allowlist.\n\nThis error happened while installing a direct dependency of C:\\Users\\crandy\\.dsh\\profiles\\web\n\nAdd the package to \"allowBuilds\" in your project's pnpm-workspace.yaml to allow it to run scripts. For example:\nallowBuilds:\n  dsh-better-sidebar@https://codeload.github.com/omdsh-dev/dsh-better-sidebar/tar.gz/b5e8665499116778fcdd0bbad2f12d7df28664d3: true\n";
        assert_eq!(
            parse_git_prepare_keys(out),
            vec!["dsh-better-sidebar@https://codeload.github.com/omdsh-dev/dsh-better-sidebar/tar.gz/b5e8665499116778fcdd0bbad2f12d7df28664d3"]
        );
    }

    #[test]
    fn git_prepare_keys_falls_back_to_error_header() {
        // 无示例块时，从错误头（fetched from URL + package NAME@VERSION）拼 key。
        let out = "[ERR_PNPM_GIT_DEP_PREPARE_NOT_ALLOWED] Failed to prepare git-hosted package fetched from \"https://codeload.github.com/a/b/tar.gz/abc123\": The git-hosted package \"b@1.2.3\" needs to execute build scripts but is not in the \"allowBuilds\" allowlist.";
        assert_eq!(parse_git_prepare_keys(out), vec!["b@https://codeload.github.com/a/b/tar.gz/abc123"]);
        // 无任何匹配 → 空。
        assert!(parse_git_prepare_keys("pnpm install completed").is_empty());
        assert!(parse_git_prepare_keys("").is_empty());
    }

    #[test]
    fn git_prepare_keys_handles_quotes_wildcard_and_dedupes() {
        // 引号 key 剥离引号；`"*"` 通配跳过；重复 key 去重；多块收集。
        let out = "allowBuilds:\n  \"*\": true\n  \"a@https://x/y/tar.gz/1\": true\n  a@https://x/y/tar.gz/1: true\n\nallowBuilds:\n  b@https://z/tar.gz/2: true\n";
        assert_eq!(
            parse_git_prepare_keys(out),
            vec!["a@https://x/y/tar.gz/1", "b@https://z/tar.gz/2"]
        );
    }

    #[test]
    fn parse_blocked_packages_merges_ignored_and_git_keys() {
        let out = "[ERR_PNPM_IGNORED_BUILDS] Ignored build scripts: ssh2@1.0.0, cpu-features@0.0.9\nFor example:\nallowBuilds:\n  dsh-x@https://codeload.github.com/a/b/tar.gz/sha1: true\n";
        assert_eq!(
            parse_blocked_packages(out),
            vec!["ssh2", "cpu-features", "dsh-x@https://codeload.github.com/a/b/tar.gz/sha1"]
        );
        // 仅 git 场景。
        let git_only = "[ERR_PNPM_GIT_DEP_PREPARE_NOT_ALLOWED] Failed to prepare git-hosted package fetched from \"https://codeload.github.com/x/y/tar.gz/zz\": The git-hosted package \"y@0.1.0\" needs to execute build scripts but is not in the \"allowBuilds\" allowlist.";
        assert_eq!(parse_blocked_packages(git_only), vec!["y@https://codeload.github.com/x/y/tar.gz/zz"]);
    }

    #[test]
    fn ensure_allow_builds_writes_git_key_verbatim() {
        // git 托管插件的 allowBuilds key 含 @ 与 ://，必须原样写入且幂等。
        let dir = tmp_dir("builds-git");
        std::fs::create_dir_all(&dir).unwrap();
        let key = "dsh-better-sidebar@https://codeload.github.com/omdsh-dev/dsh-better-sidebar/tar.gz/b5e8665499116778fcdd0bbad2f12d7df28664d3";
        assert_eq!(ensure_allow_builds(&dir, &[key]).unwrap(), true);
        let content = std::fs::read_to_string(dir.join("pnpm-workspace.yaml")).unwrap();
        assert!(content.contains(&format!("  {key}: true")), "{content}");
        // 幂等：第二次调用不再修改。
        assert_eq!(ensure_allow_builds(&dir, &[key]).unwrap(), false);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_ignored_build_packages_extracts_names() {
        let out = "[ERR_PNPM_IGNORED_BUILDS] Ignored build scripts: cloudflared@0.7.3, cpu-features@0.0.10, ssh2@1.17.0";
        assert_eq!(
            parse_ignored_build_packages(out),
            vec!["cloudflared", "cpu-features", "ssh2"]
        );
        // Ignored builds 变体 + scoped 包 + 去重。
        let out2 = "Ignored builds: @scope/a@1.2.3, b@0.1.0, @scope/a@1.2.3";
        assert_eq!(
            parse_ignored_build_packages(out2),
            vec!["@scope/a", "b"]
        );
        // 无匹配。
        assert!(parse_ignored_build_packages("pnpm install completed").is_empty());
        assert!(parse_ignored_build_packages("").is_empty());
        // 空 token 容错。
        let out3 = "Ignored build scripts: a@1.0.0, , b@2.0.0";
        assert_eq!(parse_ignored_build_packages(out3), vec!["a", "b"]);
    }

    #[test]
    fn ensure_allow_builds_replaces_placeholder_and_inserts_missing() {
        // 复现用户实际场景：allowBuilds 已有 "*" 与占位值条目。
        let dir = tmp_dir("builds1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("pnpm-workspace.yaml"),
            "packages:\n  - .\n\nnodeLinker: hoisted\nautoInstallPeers: false\n\nallowBuilds:\n  \"*\": true\n  cloudflared: set this to true or false\n  cpu-features: set this to true or false\n",
        )
        .unwrap();
        assert_eq!(
            ensure_allow_builds(&dir, &["cloudflared", "cpu-features", "ssh2"]).unwrap(),
            true
        );
        let content = std::fs::read_to_string(dir.join("pnpm-workspace.yaml")).unwrap();
        assert!(content.contains("  cloudflared: true\n"), "{content}");
        assert!(content.contains("  \"*\": true\n"), "{content}");
        // 缺失的包被插入（在 allowBuilds 段内）。
        assert!(content.contains("  ssh2: true\n"), "{content}");
        // 占位值被替换。
        assert!(!content.contains("set this to true or false"), "{content}");
        // 幂等：全部已是 true 时不再修改。
        assert_eq!(
            ensure_allow_builds(&dir, &["cloudflared", "cpu-features", "ssh2"]).unwrap(),
            false
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ensure_allow_builds_appends_new_section() {
        let dir = tmp_dir("builds2");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("pnpm-workspace.yaml"),
            "packages:\n  - .\n\nnodeLinker: hoisted\n",
        )
        .unwrap();
        assert_eq!(ensure_allow_builds(&dir, &["ssh2", "cpu-features"]).unwrap(), true);
        let content = std::fs::read_to_string(dir.join("pnpm-workspace.yaml")).unwrap();
        assert!(content.contains("allowBuilds:\n  ssh2: true\n  cpu-features: true\n"), "{content}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ensure_allow_builds_creates_missing_file_and_ignores_empty() {
        let dir = tmp_dir("builds3");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(ensure_allow_builds(&dir, &["a"]).unwrap(), true);
        let content = std::fs::read_to_string(dir.join("pnpm-workspace.yaml")).unwrap();
        assert_eq!(content, "allowBuilds:\n  a: true\n");
        // 空包列表：不修改。
        assert_eq!(ensure_allow_builds(&dir, &[]).unwrap(), false);
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---------- strip_env_prefix ----------

    #[test]
    fn install_args_prepends_add() {
        // 回归护栏：安装命令必须带 add 动词（缺省会触发 ERR_PNPM_RECURSIVE_EXEC_FIRST_FAIL）。
        let specs = vec!["@linxin666/dsh-web-ui-all".to_string(), "b".to_string()];
        assert_eq!(install_args(&specs), vec!["add", "@linxin666/dsh-web-ui-all", "b"]);
        assert_eq!(install_args(&[]), vec!["add"]);
    }

    #[test]
    fn env_prefix_stripped() {
        assert_eq!(
            strip_env_prefix("$env:PNPM_ALLOW_BUILDS=\"*\"; dsh plugin add x"),
            "dsh plugin add x"
        );
        assert_eq!(
            strip_env_prefix("$env:A='1';$env:B=\"2\";  github:a/b"),
            "github:a/b"
        );
        assert_eq!(strip_env_prefix("github:a/b"), "github:a/b");
        assert_eq!(strip_env_prefix("  dsh plugin add x"), "dsh plugin add x");
    }

    // ---------- reconcile_profile_bundles ----------

    #[test]
    fn reconcile_prunes_only_unresolvable_bundles() {
        let base = tmp_dir("recon1");
        let install = base.join("kernel");
        let home = base.join("home");
        // 内核 node_modules：只提供 dsh-base（模拟预构建内核）。
        std::fs::create_dir_all(install.join("node_modules/@deepseek-ai/dsh-base")).unwrap();
        std::fs::write(
            install.join("node_modules/@deepseek-ai/dsh-base/package.json"),
            r#"{"name":"@deepseek-ai/dsh-base","version":"1.0.0"}"#,
        )
        .unwrap();
        // profile web：bundles 含可解析的 dsh-base 与不可解析的 dsh-tauri（其它发行版残留）。
        let pdir = home.join("profiles/web");
        std::fs::create_dir_all(&pdir).unwrap();
        std::fs::write(
            pdir.join("package.json"),
            r#"{
  "name": "dsh-profile-web",
  "private": true,
  "dependencies": { "some-plugin": "1.0.0" },
  "dsh": { "profile": { "bundles": ["@deepseek-ai/dsh-base", "dsh-tauri"] } }
}"#,
        )
        .unwrap();

        let removed = reconcile_profile_bundles(&install, &home);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].0, "web");
        assert_eq!(removed[0].1, vec!["dsh-tauri"]);

        // 写回后：bundle 只保留可解析项，其它字段（依赖等）原样保留。
        let text = std::fs::read_to_string(pdir.join("package.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let bundles: Vec<&str> = v
            .pointer("/dsh/profile/bundles")
            .and_then(|b| b.as_array())
            .map(|arr| arr.iter().filter_map(|x| x.as_str()).collect())
            .unwrap();
        assert_eq!(bundles, vec!["@deepseek-ai/dsh-base"]);
        assert_eq!(
            v.pointer("/dependencies/some-plugin").and_then(|x| x.as_str()),
            Some("1.0.0")
        );

        // 幂等：再次执行无改动。
        assert!(reconcile_profile_bundles(&install, &home).is_empty());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn reconcile_keeps_bundle_resolvable_via_profiles_node_modules() {
        // 模拟 pnpm workspace hoist：bundle 装在 profiles/node_modules（上溯查找命中）。
        let base = tmp_dir("recon2");
        let install = base.join("kernel");
        let home = base.join("home");
        std::fs::create_dir_all(install.join("node_modules")).unwrap();
        std::fs::create_dir_all(home.join("profiles/node_modules/@deepseek-ai/dsh-web-app")).unwrap();
        std::fs::write(
            home.join("profiles/node_modules/@deepseek-ai/dsh-web-app/package.json"),
            r#"{"name":"@deepseek-ai/dsh-web-app","version":"1.0.0"}"#,
        )
        .unwrap();
        let pdir = home.join("profiles/web");
        std::fs::create_dir_all(&pdir).unwrap();
        std::fs::write(
            pdir.join("package.json"),
            r#"{"name":"dsh-profile-web","private":true,"dsh":{"profile":{"bundles":["@deepseek-ai/dsh-web-app"]}}}"#,
        )
        .unwrap();

        // hoist 到 profiles/node_modules 的 bundle 应视为可解析，不产生移除。
        assert!(reconcile_profile_bundles(&install, &home).is_empty());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn reconcile_skips_missing_home_and_unparseable_profile() {
        let base = tmp_dir("recon3");
        let install = base.join("kernel");
        let home = base.join("home");
        std::fs::create_dir_all(&install).unwrap();
        // home 不存在 → no-op。
        assert!(reconcile_profile_bundles(&install, &home).is_empty());
        // profile 的 package.json 无法解析 → 跳过（不 panic、不写回）。
        let pdir = home.join("profiles/web");
        std::fs::create_dir_all(&pdir).unwrap();
        std::fs::write(pdir.join("package.json"), "not json").unwrap();
        assert!(reconcile_profile_bundles(&install, &home).is_empty());
        // 无 bundles 字段的 profile → 跳过。
        std::fs::write(pdir.join("package.json"), r#"{"name":"x"}"#).unwrap();
        assert!(reconcile_profile_bundles(&install, &home).is_empty());
        std::fs::remove_dir_all(&base).ok();
    }
}
