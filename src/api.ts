// Tauri 命令与事件封装。

import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  BalanceResult,
  DetectResult,
  AppConfig,
  AppUpdateState,
  LogLine,
  PipelineEvent,
  PluginList,
  PluginOpResult,
  PluginUpdates,
  PrebuiltRelease,
  Rect,
  ToolStatus,
  UninstallPreview,
  UpdateCheckResult,
  WebStatus,
} from "./types";

async function runPipeline<T = void>(
  cmd: string,
  args: Record<string, unknown>,
  onEvent: (e: PipelineEvent) => void,
): Promise<T> {
  const channel = new Channel<PipelineEvent>();
  channel.onmessage = onEvent;
  return invoke<T>(cmd, { ...args, channel });
}

export const api = {
  getConfig: () => invoke<AppConfig>("get_config"),
  saveConfig: (cfg: AppConfig) => invoke<void>("save_config", { cfg }),
  getDefaultParentDir: () => invoke<string>("default_parent_dir"),
  /** 同步原生窗口主题（标题栏深/浅色跟随软件主题）："dark" | "light" | "auto"。 */
  setWindowTheme: (theme: string) => invoke<void>("set_window_theme", { theme }),
  detectState: () => invoke<DetectResult>("detect_state"),
  detectTools: () => invoke<ToolStatus[]>("detect_tools"),
  getBalance: () => invoke<BalanceResult>("get_balance"),
  install: (mode: string, version: string | null, onEvent: (e: PipelineEvent) => void) =>
    runPipeline("install", { mode, version }, onEvent),
  listPrebuiltReleases: () => invoke<PrebuiltRelease[]>("list_prebuilt_releases"),
  checkForUpdates: () => invoke<UpdateCheckResult>("check_for_updates"),
  update: (onEvent: (e: PipelineEvent) => void) => runPipeline("update", {}, onEvent),
  repairInstall: (kernelId: string | null, onEvent: (e: PipelineEvent) => void) =>
    runPipeline("repair_install", { kernelId }, onEvent),
  uninstallPreview: (onEvent: (e: PipelineEvent) => void) =>
    runPipeline<UninstallPreview>("uninstall_preview", {}, onEvent),
  cancelUninstallPreview: () => invoke<void>("cancel_uninstall_preview"),
  uninstall: (selected: string[], onEvent: (e: PipelineEvent) => void) =>
    runPipeline("uninstall", { selected }, onEvent),
  startWeb: (kernelId: string | null, onEvent: (e: PipelineEvent) => void) =>
    runPipeline("start_web", { kernelId }, onEvent),
  stopWeb: () => invoke<void>("stop_web"),
  getWebUrl: () => invoke<string>("get_web_url"),
  openTerminal: () => invoke<void>("open_terminal"),
  openWebUi: () => invoke<void>("open_web_ui"),
  openExternal: (url: string) => invoke<void>("open_external", { url }),
  /** 程序内原生内嵌 Webview（承载 DSH 界面；坐标为逻辑像素，相对主窗口客户区）。 */
  webviewEmbed: {
    create: (url: string, rect: Rect) =>
      invoke<void>("embed_webview_create", { url, x: rect.x, y: rect.y, width: rect.width, height: rect.height }),
    navigate: (url: string) => invoke<void>("embed_webview_navigate", { url }),
    reposition: (rect: Rect) =>
      invoke<void>("embed_webview_reposition", { x: rect.x, y: rect.y, width: rect.width, height: rect.height }),
    show: () => invoke<void>("embed_webview_show"),
    hide: () => invoke<void>("embed_webview_hide"),
    close: () => invoke<void>("embed_webview_close"),
  },
  getLogs: () => invoke<string[]>("get_logs"),
  getLogDir: () => invoke<string>("get_log_dir"),
  clearLogs: () => invoke<void>("clear_logs"),
  // ── 控制面板自身更新 ──
  getAppUpdateState: () => invoke<AppUpdateState>("get_app_update_state"),
  checkAppUpdate: () => invoke<AppUpdateState>("check_app_update"),
  /** 升级准备：解压就绪包并生成 updater.cmd，返回目标版本。完成后调用 restartToUpdate。 */
  applyAppUpdate: (onEvent: (e: PipelineEvent) => void) =>
    runPipeline<string>("apply_app_update", {}, onEvent),
  restartToUpdate: () => invoke<void>("restart_to_update"),
  pluginList: (profile: string) => invoke<PluginList>("plugin_list", { profile }),
  pluginProfiles: () => invoke<string[]>("plugin_profiles"),
  pluginCheckUpdates: (profile: string) => invoke<PluginUpdates>("plugin_check_updates", { profile }),
  pluginInstall: (input: string, profile: string, onEvent: (e: PipelineEvent) => void) =>
    runPipeline<PluginOpResult>("plugin_install", { input, profile }, onEvent),
  pluginUpdate: (specs: string[], profile: string, onEvent: (e: PipelineEvent) => void) =>
    runPipeline<PluginOpResult>("plugin_update", { specs, profile }, onEvent),
  pluginRemove: (specs: string[], profile: string, onEvent: (e: PipelineEvent) => void) =>
    runPipeline<PluginOpResult>("plugin_remove", { specs, profile }, onEvent),
};

export function onWebStatus(cb: (s: WebStatus) => void): Promise<() => void> {
  return listen<WebStatus>("web-status", (e) => cb(e.payload));
}

export function onUpdateChecked(cb: (r: UpdateCheckResult) => void): Promise<() => void> {
  return listen<UpdateCheckResult>("update-checked", (e) => cb(e.payload));
}

export function onPluginUpdatesChecked(cb: (u: PluginUpdates) => void): Promise<() => void> {
  return listen<PluginUpdates>("plugin-updates-checked", (e) => cb(e.payload));
}

export function onLogLine(cb: (l: LogLine) => void): Promise<() => void> {
  return listen<LogLine>("log-line", (e) => cb(e.payload));
}

export function onAppUpdateState(cb: (s: AppUpdateState) => void): Promise<() => void> {
  return listen<AppUpdateState>("app-update-state", (e) => cb(e.payload));
}

/** 已有完整更新包（本次启动即进入升级）；payload 为目标版本号。 */
export function onAppUpdateReady(cb: (version: string) => void): Promise<() => void> {
  return listen<string>("app-update-ready", (e) => cb(e.payload));
}
