//! 控制面板界面语言的全局设置与后端文案目录。
//!
//! - 语言优先级：`config.language`（"auto" / "zh-CN" / "en"）→ 应用启动与保存配置时同步到全局；
//!   "auto" 跟随系统语言（与前端 detectSystemLang 一致）；
//! - `AppError::friendly()`、各模块返回给前端的错误/提示文案、控制面板自身日志（Logger 正文）
//!   与步骤标题均按当前语言输出中 / 英文；
//! - 技术性输出（git / pnpm 等外部命令输出、DSH 进程输出）保持原文，不做翻译。

use std::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Zh,
    En,
}

static CURRENT: RwLock<Lang> = RwLock::new(Lang::Zh);

pub fn set_lang(l: Lang) {
    if let Ok(mut g) = CURRENT.write() {
        *g = l;
    }
}

pub fn get_lang() -> Lang {
    match CURRENT.read() {
        Ok(g) => *g,
        Err(_) => Lang::Zh,
    }
}

/// 从配置的 language 字符串解析（"zh-CN"（含 "zh"）→ Zh；"auto" 跟随系统语言；
/// 其余含空值 → En）。系统语言解析失败时回退 En。
pub fn lang_from_config(s: &str) -> Lang {
    let v = s.trim().to_ascii_lowercase();
    if v == "zh-cn" || v == "zh" {
        return Lang::Zh;
    }
    if v == "auto" {
        // auto：跟随系统语言（与前端 detectSystemLang 的口径一致）。
        let locale = sys_locale::get_locale().unwrap_or_default().to_ascii_lowercase();
        return if locale.starts_with("zh") { Lang::Zh } else { Lang::En };
    }
    Lang::En
}

/// 后端文案目录：返回 (中文, English)。未知 key 返回 None（由调用方原样透出）。
fn catalog(key: &str) -> Option<(&'static str, &'static str)> {
    Some(match key {
        // ── 插件管理 ──
        "plugin.running" => (
            "web 服务正在运行，请先停止服务后再更新/卸载插件；安装新插件不受限制",
            "The web service is running. Stop it first before updating or removing plugins; installing new plugins is still allowed.",
        ),
        "plugin.missing.update.spec" => ("缺少要更新的插件标识", "Missing plugin identifier to update."),
        "plugin.missing.remove.selection" => ("请先选择要卸载的插件", "Please select plugins to remove first."),
        "plugin.profile.invalid" => ("非法的 profile 名称：{0}", "Invalid profile name: {0}"),
        "plugin.input.empty" => (
            "请输入要安装的插件标识或完整命令",
            "Enter a plugin identifier or a full command to install.",
        ),
        "plugin.input.unrecognized" => (
            "无法识别输入：{0}。支持 npm 包名、github:owner/repo[#ref]、GitHub 链接（含 .tar.gz 压缩包链接），或完整 dsh plugin add 命令",
            "Unrecognized input: {0}. Supported: npm package name, github:owner/repo[#ref], GitHub links (including .tar.gz archive URLs), or a full `dsh plugin add` command.",
        ),
        "plugin.spec.empty" => ("插件标识为空", "Plugin identifier is empty."),
        "plugin.spec.invalid.char" => ("插件标识包含非法字符：{0}", "Plugin identifier contains invalid characters: {0}"),
        "plugin.github.bad.link" => (
            "GitHub 链接格式应为 https://github.com/owner/repo[#分支/tag/commit]，收到：{0}",
            "GitHub link should look like https://github.com/owner/repo[#branch/tag/commit], got: {0}",
        ),
        "plugin.github.missing.owner" => ("GitHub 链接缺少 owner 或 repo：{0}", "GitHub link is missing owner or repo: {0}"),
        "plugin.github.inner.path" => (
            "GitHub 链接包含仓库内路径，请改用 github:owner/repo#<分支或子目录> 形式",
            "GitHub link contains a path inside the repository; use github:owner/repo#<branch-or-subdir> instead.",
        ),
        "plugin.github.empty.ref" => ("GitHub 链接 # 后为空：{0}", "GitHub link has an empty ref after #: {0}"),
        "plugin.github.bad.spec" => (
            "github 标识格式应为 github:owner/repo[#分支/tag/commit/子目录]，收到：{0}",
            "The github spec should look like github:owner/repo[#branch/tag/commit/subdir], got: {0}",
        ),
        "plugin.spec.unrecognized" => ("无法识别插件标识：{0}", "Unrecognized plugin identifier: {0}"),
        "plugin.npm.bad.scope" => ("npm 包名格式应为 @scope/name，收到：{0}", "npm package name should look like @scope/name, got: {0}"),
        "plugin.spec.ownerrepo.bad" => ("无法识别插件标识（owner/repo 形式）：{0}", "Unrecognized plugin identifier (owner/repo form): {0}"),
        "plugin.spec.hash.empty" => ("插件标识 # 后为空：{0}", "Plugin identifier has an empty ref after #: {0}"),
        "plugin.cmd.missing.profile" => ("--profile 后缺少 profile 名称", "Missing profile name after --profile"),
        "plugin.cmd.no.verb" => (
            "未找到插件操作（add/update/remove）。完整命令示例：dsh plugin --profile web add <包名>",
            "No plugin operation found (add/update/remove). Full command example: dsh plugin --profile web add <package>",
        ),
        "plugin.cmd.add.missing.spec" => (
            "add 后缺少插件标识（npm 包名 / github:owner/repo / GitHub 链接（含 .tar.gz 压缩包链接））",
            "Missing plugin identifier after add (npm package name / github:owner/repo / GitHub link (including .tar.gz archive URLs)).",
        ),
        "plugin.cmd.install.verb" => (
            "install 会安装 profile 的全部依赖而非单个插件，请使用 add <包名> 安装指定插件",
            "`install` installs all dependencies of the profile rather than a single plugin; use `add <package>` instead.",
        ),
        "plugin.cmd.remove.verb" => (
            "检测到卸载操作。请在下方已安装列表中勾选插件后点击「卸载所选」（或「全部卸载」）",
            "Detected a remove operation. Select plugins in the installed list below and click \"Remove selected\" (or \"Remove all\").",
        ),
        "plugin.cmd.update.verb" => (
            "检测到更新操作。请在下方列表中点击对应插件的「更新」按钮",
            "Detected an update operation. Click the \"Update\" button of the corresponding plugin in the list below.",
        ),
        "plugin.cmd.query.verb" => (
            "检测到查询命令。安装插件请使用 add：dsh plugin --profile web add <包名>",
            "Detected a query command. Use `add` to install plugins: dsh plugin --profile web add <package>",
        ),
        "plugin.cmd.unsupported" => ("不支持的插件操作：{0}。安装请使用 add 命令", "Unsupported plugin operation: {0}. Use `add` to install."),
        // ── 插件操作结果 / 自动重试提示 ──
        "plugin.op.install" => ("安装插件", "Install plugin"),
        "plugin.op.update" => ("更新插件", "Update plugin"),
        "plugin.op.remove" => ("卸载插件", "Remove plugin"),
        "plugin.op.repair" => ("修复 profile", "Repair profile"),
        "plugin.op.ok" => ("{0}成功：{1}", "{0} succeeded: {1}"),
        "plugin.op.failed" => ("{0}失败（退出码 {1}）：{2}", "{0} failed (exit code {1}): {2}"),
        "plugin.op.no.output" => ("（无输出）", "(no output)"),
        "plugin.op.dsh.switch" => (
            "检测到「dsh」命令无法识别，已自动改用 pnpm dsh 执行（已保存到配置，后续命令保持一致）",
            "\"dsh\" is not recognized; automatically switched to `pnpm dsh` (saved to settings, kept for later commands).",
        ),
        "plugin.op.dsh.switch.savefail" => ("切换 pnpm dsh 后保存配置失败：{0}", "Failed to save settings after switching to pnpm dsh: {0}"),
        "plugin.op.builds.parse" => (
            "检测到 pnpm 拦截构建脚本，但无法从输出解析被拦截的包名",
            "pnpm blocked build scripts, but blocked package names could not be parsed from the output.",
        ),
        "plugin.op.builds.writefail" => ("写入 pnpm-workspace.yaml 失败：{0}", "Failed to write pnpm-workspace.yaml: {0}"),
        "plugin.op.builds.note1" => (
            "检测到 pnpm 拦截插件构建脚本，已将包 {0} 显式加入 profile 的 allowBuilds 并自动重试",
            "pnpm blocked plugin build scripts; added {0} to the profile's allowBuilds and retried automatically.",
        ),
        "plugin.op.builds.note2" => (
            "检测到 pnpm 拦截插件构建脚本（包 {0} 已在 allowBuilds 中），正在自动重试",
            "pnpm blocked plugin build scripts ({0} is already in allowBuilds); retrying automatically.",
        ),
        // ── 修复安装 ──
        "repair.env.missing" => (
            "运行环境缺失或版本不满足要求（{0}）。请先按「安装指引」完成环境安装，再执行修复安装。",
            "Required tools are missing or below the minimum version ({0}). Complete the environment setup following the install guide, then repair again.",
        ),
        "repair.fetch.failed" => ("git fetch 失败：无法获取远程代码", "git fetch failed: could not fetch remote code."),
        "repair.reset.failed" => ("git reset --hard 失败", "git reset --hard failed."),
        "repair.clean.failed" => ("git clean 失败", "git clean failed."),
        "repair.install.failed" => ("pnpm install 失败", "pnpm install failed."),
        "repair.build.failed" => ("pnpm run build 失败", "pnpm run build failed."),
        "repair.nm.delete.failed" => (
            "删除 node_modules 失败：{0}。请关闭占用该目录的程序后重试。",
            "Failed to delete node_modules: {0}. Close programs using this directory and retry.",
        ),
        "repair.no.parent" => ("无法确定安装目录的父目录", "Cannot determine the parent directory of the installation."),
        "repair.dir.delete.failed" => (
            "删除安装目录失败：{0}。请关闭占用该目录的程序（如终端、编辑器）后重试。",
            "Failed to delete the installation directory: {0}. Close programs using it (e.g. terminal, editor) and retry.",
        ),
        "repair.clone.failed" => ("重新克隆失败", "Re-cloning failed."),
        // ── 更新 ──
        "update.pull.hint" => (
            " 提示：若本地存在未提交的改动或网络异常，请先手动处理（可在终端中执行 git pull 查看原因），再重试更新。",
            " Tip: if there are uncommitted local changes or a network issue, resolve them first (run `git pull` in a terminal to inspect), then retry the update.",
        ),
        // ── 卸载预览扫描 ──
        "uninstall.scan.progress" => (
            "正在统计 {0}：{1} 项 / {2}",
            "Scanning {0}: {1} items / {2}",
        ),
        "uninstall.scan.cancelled" => (
            "已取消统计",
            "Scan cancelled",
        ),
        // ── 步骤标题（与前端 i18n.ts 的 step.* 一一对应）──
        "step.clone" => ("克隆仓库（git clone）", "Clone repository (git clone)"),
        "step.install" => ("安装依赖（pnpm install）", "Install dependencies (pnpm install)"),
        "step.build" => ("构建（pnpm run build）", "Build (pnpm run build)"),
        "step.pull" => ("拉取最新代码（git pull --ff-only）", "Pull latest code (git pull --ff-only)"),
        "step.web" => ("启动 web 服务（pnpm dsh web）", "Start web service (pnpm dsh web)"),
        "step.repair-fetch" => ("拉取远程引用（git fetch origin）", "Fetch remote refs (git fetch origin)"),
        "step.repair-reset" => ("重置到远程默认分支（git reset --hard）", "Reset to remote default branch (git reset --hard)"),
        "step.repair-clean" => ("清理多余文件（git clean）", "Clean extra files (git clean)"),
        "step.repair-install" => ("安装依赖（pnpm install）", "Install dependencies (pnpm install)"),
        "step.repair-build" => ("构建（pnpm run build）", "Build (pnpm run build)"),
        "step.repair-clean-nm" => ("删除 node_modules（深度重建）", "Delete node_modules (deep rebuild)"),
        "step.repair-remove" => ("删除损坏的安装目录（保底重装）", "Delete corrupted installation directory (last-resort reinstall)"),
        "step.repair-clone" => ("重新克隆仓库（git clone）", "Re-clone repository (git clone)"),
        "step.delete" => ("删除 {0}", "Delete {0}"),
        // ── 控制面板自身日志（Logger 正文，按界面语言输出）──
        "log.boot" => ("=== DSH Control Panel v{0} 启动 ===", "=== DSH Control Panel v{0} started ==="),
        "log.os" => (
            "操作系统: {0} · 日志文件: {1}",
            "OS: {0} · log file: {1}",
        ),
        "log.exit" => ("=== DSH Control Panel 退出 ===", "=== DSH Control Panel exited ==="),
        "log.config_loaded" => (
            "配置加载完成: install_dir={0} auto_check={1} language={2}",
            "Config loaded: install_dir={0} auto_check={1} language={2}",
        ),
        "log.settings_saved" => (
            "⚙ 保存设置（自动检测 / 间隔 / 语言等）",
            "⚙ Settings saved (auto-check / interval / language, etc.)",
        ),
        "log.picked_dir" => ("📁 选择目录: {0}", "📁 Selected directory: {0}"),
        "log.detect_state" => (
            "🔎 状态探测: installed={0} version={1} commit={2} running={3}",
            "🔎 State probe: installed={0} version={1} commit={2} running={3}",
        ),
        "log.tools_summary" => ("🔧 环境检测: {0}", "🔧 Runtime check: {0}"),
        "log.manual_scan_found" => (
            "🔎 扫描手动安装: 发现 {0} 个候选",
            "🔎 Manual install scan: found {0} candidate(s)",
        ),
        "log.manual_scan_item" => ("   • {0}", "   • {0}"),
        "log.adopt_done" => (
            "✅ 已采用手动安装: {0} (version={1} commit={2})",
            "✅ Adopted manual installation: {0} (version={1} commit={2})",
        ),
        "log.update_check" => (
            "🔎 检测更新: behind={0} update_available={1} subject={2}",
            "🔎 Update check: behind={0} update_available={1} subject={2}",
        ),
        "log.open_terminal" => ("🖥 打开终端: {0}", "🖥 Opened terminal: {0}"),
        "log.open_browser" => ("🌐 在系统浏览器打开 {0}", "🌐 Opened {0} in the system browser"),
        "log.open_external" => ("🔗 打开外部链接: {0}", "🔗 Opened external link: {0}"),
        "log.dsh_probe_ok" => (
            "🔌 dsh 命令探测：全局 dsh 可用（插件命令以 dsh 执行）",
            "🔌 dsh probe: global dsh available (plugin commands run via dsh)",
        ),
        "log.dsh_probe_fallback" => (
            "🔌 dsh 命令探测：全局 dsh 不可用（插件等 dsh 命令将以 pnpm dsh 执行）",
            "🔌 dsh probe: global dsh unavailable (dsh commands will run via `pnpm dsh`)",
        ),
        "log.step_start" => ("▶ {0}: {1} {2}", "▶ {0}: {1} {2}"),
        "log.step_done" => ("✓ {0} 完成（退出码 {1}）", "✓ {0} finished (exit code {1})"),
        "log.install_done" => (
            "✅ 安装完成（目录: {0}）",
            "✅ Installation complete (directory: {0})",
        ),
        "log.update_done" => ("✅ 更新完成", "✅ Update complete"),
        "log.plugin_op_start" => (
            "🔌 {0}: {1}（目录 {2}）",
            "🔌 {0}: {1} (directory {2})",
        ),
        "log.uninstall_delete" => ("🗑 删除 {0}", "🗑 Deleting {0}"),
        "log.uninstall_delete_failed" => (
            "删除 {0} 失败：{1}。请关闭占用该路径的程序（如终端、编辑器）后重试。",
            "Failed to delete {0}: {1}. Close programs using this path (e.g. terminal, editor) and retry.",
        ),
        "log.uninstall_done" => ("✅ 卸载完成", "✅ Uninstall complete"),
        "log.repair_start" => ("🛠 修复安装开始…", "🛠 Repair install started…"),
        "log.repair_done" => ("✅ 修复安装完成", "✅ Repair install complete"),
        "log.repair_kill_ok" => (
            "🧹 已清理安装目录相关的残留进程",
            "🧹 Cleaned up stray processes related to the install directory",
        ),
        "log.repair_kill_fail" => (
            "⚠ 清理残留进程失败（{0}，已跳过，不影响后续步骤）",
            "⚠ Failed to clean stray processes ({0}; skipped, does not affect later steps)",
        ),
        "log.repair_step_done" => ("✓ {0} 完成", "✓ {0} finished"),
        "log.repair_step_failed" => ("✗ {0} 失败（退出码 {1}）", "✗ {0} failed (exit code {1})"),
        "log.repair_builds_parse" => (
            "检测到 pnpm 拦截构建脚本，但无法解析被拦截的包名",
            "pnpm blocked build scripts, but blocked package names could not be parsed.",
        ),
        "log.repair_builds_note1" => (
            "检测到 pnpm 拦截构建脚本，已将 {0} 加入 allowBuilds 并自动重试",
            "pnpm blocked build scripts; added {0} to allowBuilds and retried automatically.",
        ),
        "log.repair_builds_note2" => (
            "检测到构建脚本被拦截，但 allowBuilds 已包含相关包，正在重试",
            "Build scripts are blocked, but the packages are already in allowBuilds; retrying.",
        ),
        "log.repair_builds_writefail" => (
            "写入 allowBuilds 失败：{0}",
            "Failed to write allowBuilds: {0}",
        ),
        "log.repair_deep" => (
            "🔄 深度重建：删除 node_modules …",
            "🔄 Deep rebuild: deleting node_modules …",
        ),
        "log.repair_profile_fix" => (
            "🔧 修复 profile「{0}」的依赖状态…",
            "🔧 Repairing dependencies of profile \"{0}\"…",
        ),
        "log.repair_profile_ok" => (
            "✅ profile「{0}」依赖就绪",
            "✅ Dependencies of profile \"{0}\" are ready",
        ),
        "log.repair_profile_fail" => (
            "⚠ profile「{0}」依赖安装失败：{1}。可稍后在「插件管理」中重试，或删除该 profile 目录后重新初始化。",
            "⚠ Failed to install dependencies of profile \"{0}\": {1}. Retry later in \"Plugin Manager\", or delete the profile directory and re-initialize it.",
        ),
        "log.repair_last_resort" => (
            "🗑 保底重装：删除安装目录 {0} …",
            "🗑 Last-resort reinstall: deleting install directory {0} …",
        ),
        "log.repair_locks" => (
            "🧹 已清理 {0} 个 git 锁 / 中断状态文件",
            "🧹 Cleaned {0} git lock / interrupted-state file(s)",
        ),
        "log.repair_no_locks" => (
            "🧹 未发现 git 锁或中断状态文件",
            "🧹 No git locks or interrupted-state files found",
        ),
        "log.repair_escalate" => (
            "🔄 常规修复失败，尝试深度重建（删除 node_modules 后重新安装依赖）…",
            "🔄 Regular repair failed; trying a deep rebuild (delete node_modules, reinstall dependencies)…",
        ),
        "log.repair_last_resort_escalate" => (
            "⚠ 深度重建仍失败，将执行保底手段：删除安装目录并重新安装（相当于卸载后重装）…",
            "⚠ Deep rebuild also failed; falling back to deleting the install directory and reinstalling (equivalent to uninstall + reinstall)…",
        ),
        "log.web_start" => (
            "🚀 启动 DeepSeek Harness web 服务（pnpm dsh web）",
            "🚀 Starting DeepSeek Harness web service (pnpm dsh web)",
        ),
        "log.web_pid" => ("进程 PID: {0}", "Process PID: {0}"),
        "log.web_stop_noop" => (
            "⏹ 停止 web 服务：未发现运行中的进程",
            "⏹ Stop web service: no running process found",
        ),
        "log.web_stop_killed" => (
            "⏹ 停止 web 服务：已结束进程 {0}",
            "⏹ Stop web service: terminated process(es) {0}",
        ),
        _ => return None,
    })
}

/// 按当前语言取后端文案（无变量；未知 key 原样返回）。
pub fn t(key: &str) -> String {
    match catalog(key) {
        Some((zh, en)) => match get_lang() {
            Lang::Zh => zh.to_string(),
            Lang::En => en.to_string(),
        },
        None => key.to_string(),
    }
}

/// 按当前语言取后端文案并填充 {0} {1} … 占位符（未知 key 原样返回）。
pub fn t_fmt(key: &str, args: &[&str]) -> String {
    let template = match catalog(key) {
        Some((zh, en)) => match get_lang() {
            Lang::Zh => zh,
            Lang::En => en,
        },
        None => return key.to_string(),
    };
    let mut out = template.to_string();
    for (i, a) in args.iter().enumerate() {
        out = out.replace(&format!("{{{i}}}"), a);
    }
    out
}

/// 按当前语言取后端文案；目录中不存在该 key 时回退 fallback 原文
/// （用于步骤标题等：新增步骤暂无翻译时保留原始标题，而不是显示裸 key）。
pub fn t_or(key: &str, fallback: &str) -> String {
    if catalog(key).is_some() {
        t(key)
    } else {
        fallback.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 这些测试读/写全局语言 `CURRENT`，并行执行会互相干扰（race）。用锁串行化。
    static LANG_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn lang_from_config_maps_zh_and_others() {
        assert_eq!(lang_from_config("zh-CN"), Lang::Zh);
        assert_eq!(lang_from_config("zh"), Lang::Zh);
        assert_eq!(lang_from_config("en"), Lang::En);
        assert_eq!(lang_from_config("fr"), Lang::En);
        assert_eq!(lang_from_config(""), Lang::En);
        // auto 跟随系统语言：结果为 Zh 或 En 之一（不 panic）。
        assert!(matches!(lang_from_config("auto"), Lang::Zh | Lang::En));
    }

    #[test]
    fn t_or_falls_back_to_fallback_for_unknown_keys() {
        let _g = LANG_LOCK.lock().unwrap();
        set_lang(Lang::Zh);
        // 已知 key：取目录文案。
        assert_eq!(t_or("step.clone", "克隆仓库（git clone）"), "克隆仓库（git clone）");
        set_lang(Lang::En);
        assert_eq!(t_or("step.clone", "克隆仓库（git clone）"), "Clone repository (git clone)");
        // 未知 key：回退原文，而不是返回裸 key。
        assert_eq!(t_or("step.brand-new", "某新步骤"), "某新步骤");
        set_lang(Lang::Zh);
    }

    #[test]
    fn t_returns_current_language_text() {
        let _g = LANG_LOCK.lock().unwrap();
        set_lang(Lang::Zh);
        assert_eq!(t("plugin.running"), "web 服务正在运行，请先停止服务后再更新/卸载插件；安装新插件不受限制");
        set_lang(Lang::En);
        assert_eq!(t("plugin.running"), "The web service is running. Stop it first before updating or removing plugins; installing new plugins is still allowed.");
        // 未知 key 原样返回。
        assert_eq!(t("no.such.key"), "no.such.key");
    }

    #[test]
    fn t_fmt_fills_positional_placeholders() {
        let _g = LANG_LOCK.lock().unwrap();
        set_lang(Lang::Zh);
        assert_eq!(
            t_fmt("plugin.profile.invalid", &["a/b"]),
            "非法的 profile 名称：a/b"
        );
        set_lang(Lang::En);
        assert_eq!(t_fmt("plugin.profile.invalid", &["a/b"]), "Invalid profile name: a/b");
        set_lang(Lang::Zh);
    }
}
