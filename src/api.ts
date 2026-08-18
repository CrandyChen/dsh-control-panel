// Tauri 命令与事件封装。

import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  DetectResult,
  AppConfig,
  LogLine,
  PipelineEvent,
  PluginList,
  PluginOpResult,
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
  pickDirectory: () => invoke<string | null>("pick_directory"),
  detectState: () => invoke<DetectResult>("detect_state"),
  detectTools: () => invoke<ToolStatus[]>("detect_tools"),
  scanManualInstalls: () => invoke<string[]>("scan_manual_installs"),
  adoptInstall: (path: string) => invoke<void>("adopt_install", { path }),
  install: (dir: string, onEvent: (e: PipelineEvent) => void) =>
    runPipeline("install", { dir }, onEvent),
  checkForUpdates: () => invoke<UpdateCheckResult>("check_for_updates"),
  update: (onEvent: (e: PipelineEvent) => void) => runPipeline("update", {}, onEvent),
  repairInstall: (onEvent: (e: PipelineEvent) => void) =>
    runPipeline("repair_install", {}, onEvent),
  uninstallPreview: () => invoke<UninstallPreview>("uninstall_preview"),
  uninstall: (selected: string[], onEvent: (e: PipelineEvent) => void) =>
    runPipeline("uninstall", { selected }, onEvent),
  startWeb: (onEvent: (e: PipelineEvent) => void) => runPipeline("start_web", {}, onEvent),
  stopWeb: () => invoke<void>("stop_web"),
  openTerminal: () => invoke<void>("open_terminal"),
  openWebUi: () => invoke<void>("open_web_ui"),
  openExternal: (url: string) => invoke<void>("open_external", { url }),
  getLogs: () => invoke<string[]>("get_logs"),
  getLogDir: () => invoke<string>("get_log_dir"),
  clearLogs: () => invoke<void>("clear_logs"),
  pluginList: (profile: string) => invoke<PluginList>("plugin_list", { profile }),
  pluginInstall: (input: string, profile: string, onEvent: (e: PipelineEvent) => void) =>
    runPipeline<PluginOpResult>("plugin_install", { input, profile }, onEvent),
  pluginUpdate: (spec: string, profile: string, onEvent: (e: PipelineEvent) => void) =>
    runPipeline<PluginOpResult>("plugin_update", { spec, profile }, onEvent),
  pluginRemove: (specs: string[], profile: string, onEvent: (e: PipelineEvent) => void) =>
    runPipeline<PluginOpResult>("plugin_remove", { specs, profile }, onEvent),
};

export function onWebStatus(cb: (s: WebStatus) => void): Promise<() => void> {
  return listen<WebStatus>("web-status", (e) => cb(e.payload));
}

export function onUpdateChecked(cb: (r: UpdateCheckResult) => void): Promise<() => void> {
  return listen<UpdateCheckResult>("update-checked", (e) => cb(e.payload));
}

export function onLogLine(cb: (l: LogLine) => void): Promise<() => void> {
  return listen<LogLine>("log-line", (e) => cb(e.payload));
}
