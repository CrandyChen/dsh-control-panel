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

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::AppHandle;

use crate::config::{self, AppConfig};
use crate::error::AppError;
use crate::logging::Logger;
use crate::process::{spawn_any, PipelineEvent};

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

/// web 服务运行中禁止更新/卸载插件（仅安装不受限）。
/// 返回 Err 时调用方应中止操作并展示该提示。
pub fn ensure_mutation_allowed() -> Result<(), String> {
    if crate::web::port_in_use(crate::config::WEB_PORT) {
        return Err(crate::i18n::t("plugin.running"));
    }
    Ok(())
}

/// 执行一次 `dsh plugin --profile <profile> <args>` 操作。
///
/// 失败自动重试（最多各一次）：
/// - 全局 `dsh` 不可识别 → 切换为 `pnpm dsh`（写回配置）；
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
    let install_dir = cfg
        .install_dir
        .clone()
        .ok_or_else(|| AppError::NotInstalled.friendly())?;
    let cwd = PathBuf::from(&install_dir);
    if !crate::detect::is_valid_repo(&cwd) {
        return Err(AppError::NotInstalled.friendly());
    }
    let pdir = profile_dir(profile)?;

    let display = format!("dsh plugin --profile {profile} {}", args.join(" "));
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

        let programs: &[&str] = if use_pnpm {
            &["pnpm.cmd", "pnpm"]
        } else {
            &["dsh.cmd", "dsh"]
        };
        let mut argv: Vec<String> = if use_pnpm {
            vec![
                "dsh".into(),
                "plugin".into(),
                "--profile".into(),
                profile.to_string(),
            ]
        } else {
            vec![
                "plugin".into(),
                "--profile".into(),
                profile.to_string(),
            ]
        };
        argv.extend(args.iter().cloned());

        let outcome = match run_capture(&programs, &argv, &cwd, &[], &step_id, channel) {
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

        // 失败 1：全局 dsh 不可识别 → 切换 pnpm dsh。
        if !use_pnpm && is_dsh_missing(&outcome.output) {
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
}
