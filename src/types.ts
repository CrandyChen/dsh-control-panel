// 前后端共享类型（与 Rust 侧 serde 字段对齐，camelCase）。

export interface AppConfig {
  installDir: string | null;
  installedVersion: string | null;
  installedCommit: string | null;
  lastUpdatedAt: string | null;
  lastCheckAt: string | null;
  updateAvailable: boolean;
  latestCommit: string | null;
  latestSubject: string | null;
  autoCheckEnabled: boolean;
  autoCheckIntervalHours: number;
  /** 是否使用 `pnpm dsh` 执行 dsh 命令（全局 dsh 不可识别时为 true，启动时自动探测）。 */
  usePnpmDsh: boolean;
  /** 插件管理默认操作的 profile 名称。 */
  pluginProfile: string;
  /** 「打开界面」默认打开方式：tab = 程序内新标签页；browser = 系统浏览器。 */
  openUiMode: "tab" | "browser";
  /** 界面语言：auto（跟随系统，非中英默认英文）/ zh-CN / en。 */
  language: "auto" | "zh-CN" | "en";
}

export interface DetectResult {
  installed: boolean;
  valid: boolean;
  built: boolean;
  version: string | null;
  running: boolean;
  installDir: string | null;
  dshHome: string;
  installedCommit: string | null;
}

/** 运行环境工具检测结果（与 Rust tools.rs 的 ToolStatus 对齐）。 */
export interface ToolStatus {
  id: string;
  name: string;
  installed: boolean;
  version: string | null;
  /** 版本是否满足最低要求（git/python 无要求时与 installed 一致）。 */
  ok: boolean;
  /** 是否必装（git/node/pnpm 为 true；python 为推荐项 false）。 */
  required: boolean;
  detail: string | null;
}

export interface UpdateCheckResult {
  updateAvailable: boolean;
  localCommit: string;
  remoteCommit: string;
  behind: number;
  subject: string;
  checkedAt: string;
}

export interface UninstallEntry {
  id: string;
  name: string;
  path: string;
  kind: "directory" | "file";
  size: number;
  items: number;
}

export interface UninstallPreview {
  entries: UninstallEntry[];
  installDir: string | null;
  dshHome: string;
}

export type PipelineEvent =
  | { type: "stepStarted"; id: string; title: string }
  | { type: "output"; step: string; stream: "stdout" | "stderr"; line: string }
  | { type: "stepFinished"; id: string; exitCode: number }
  | { type: "error"; message: string }
  | { type: "finished"; ok: boolean };

export type WebStatus = "idle" | "starting" | "ready" | "stopped" | "error";

/** 内嵌浏览器 Tab（纯前端 iframe 模型）。 */
export interface BrowserTab {
  id: string;
  title: string;
  url: string;
}

export interface LogLine {
  level: string;
  text: string;
  ts: string;
}

/** 插件条目（来自 profile 清单的 dependencies）。 */
export interface PluginEntry {
  /** 依赖 key（pnpm 记录的精确标识，更新/卸载时原样使用）。 */
  key: string;
  /** 依赖 value（版本范围或 git spec）。 */
  spec: string;
  /** 是否在 dsh.profile.bundles 激活层栈中（组合包插件）。 */
  isBundle: boolean;
  /** 已安装的实际版本（读 node_modules/<key>/package.json；未安装时为 null）。 */
  version: string | null;
}

/** 指定 profile 的插件列表。 */
export interface PluginList {
  profile: string;
  profileDir: string;
  entries: PluginEntry[];
  /** 内置组合包（非依赖，随 dsh 安装提供，只读不可卸载）。 */
  builtinBundles: string[];
  /** profile 是否已初始化（存在 package.json）。 */
  initialized: boolean;
  /** 当前是否使用 `pnpm dsh` 执行命令。 */
  usePnpmDsh: boolean;
}

/** 插件操作结果（详情走 Channel 事件流）。 */
export interface PluginOpResult {
  ok: boolean;
  message: string;
  action: string;
}

/** 长任务进行中的阶段（驱动按钮禁用与 Steps 展示）。 */
export type Phase =
  | "idle"
  | "installing"
  | "updating"
  | "uninstalling"
  | "previewing"
  | "checking"
  | "starting"
  | "stopping"
  | "repairing";

/** 格式化字节数。 */
export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v.toFixed(1)} ${units[i]}`;
}
