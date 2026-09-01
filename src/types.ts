// 前后端共享类型（与 Rust 侧 serde 字段对齐，camelCase）。

/** DSH 安装方式：source = 从官方源码安装（需 Git）；prebuilt = 预构建内核（免 Git/pnpm）。 */
export type InstallMode = "source" | "prebuilt";

export interface AppConfig {
  installDir: string | null;
  /** 当前安装方式（source / prebuilt）。 */
  installMode: InstallMode;
  installedVersion: string | null;
  installedCommit: string | null;
  lastUpdatedAt: string | null;
  lastCheckAt: string | null;
  updateAvailable: boolean;
  latestCommit: string | null;
  latestSubject: string | null;
  autoCheckEnabled: boolean;
  autoCheckIntervalHours: number;
  /** 是否启用定时自动检测插件更新（默认 profile）。 */
  pluginAutoCheckEnabled: boolean;
  /** 插件自动检测间隔（小时）。 */
  pluginAutoCheckIntervalHours: number;
  /** 是否使用 `pnpm dsh` 执行 dsh 命令（全局 dsh 不可识别时为 true，启动时自动探测）。 */
  usePnpmDsh: boolean;
  /** 插件管理默认操作的 profile 名称。 */
  pluginProfile: string;
  /** 「打开界面」默认打开方式：tab = 程序内新标签页；browser = 系统浏览器。 */
  openUiMode: "tab" | "browser";
  /** 界面主题：auto（跟随系统，随 OS 深/浅实时切换）/ light / dark。持久化保存。 */
  theme: "auto" | "light" | "dark";
  /** 界面语言：auto（跟随系统，非中英默认英文）/ zh-CN / en。 */
  language: "auto" | "zh-CN" | "en";
  /** 已安装的内核版本注册表（多版本相互独立，共享 ~/.dsh 数据目录）。 */
  installedKernels: KernelInstall[];
  /** 最近一次正常启动的内核 id（启动选择框的默认勾选项；首次为最新预构建内核）。 */
  lastStartedKernelId: string | null;
}

/** 一个已安装的 DSH 内核版本。 */
export interface KernelInstall {
  /** 唯一标识：prebuilt-<version>；源码模式固定为 "source"。 */
  id: string;
  mode: InstallMode;
  version: string;
  installDir: string;
  commit: string | null;
  installedAt: string;
}

/** 一个可安装的预构建内核版本（来自 GitHub release 列表）。 */
export interface PrebuiltRelease {
  tag: string;
  version: string;
  url: string;
  size: number;
  prerelease: boolean;
  publishedAt: string;
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

/** 运行环境工具检测结果（与 Rust tools.rs 的 ToolStatus 对齐；现仅检测源码模式的 Git）。 */
export interface ToolStatus {
  id: string;
  name: string;
  installed: boolean;
  version: string | null;
  /** 版本是否满足最低要求（git 无要求时与 installed 一致）。 */
  ok: boolean;
  /** 是否必装（git 为 true；预构建模式不返回此项）。 */
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
  | { type: "finished"; ok: boolean }
  | {
      /** 预构建内核下载进度（received/total 为字节；speedBps 为平均速度）。 */
      type: "downloadProgress";
      step: string;
      received: number;
      total: number;
      speedBps: number;
    }
  | { type: "runtimeDone" };

export type WebStatus = "idle" | "starting" | "ready" | "stopped" | "error";

/**
 * 内嵌浏览器 Tab。
 * - native=true：DSH 界面，由程序内**原生子 Webview** 承载（顶层导航，规避认证/点击劫持限制）；
 * - native=false：普通 iframe（安装指引 blob、自定义 URL）。
 */
export interface BrowserTab {
  id: string;
  title: string;
  url: string;
  /** 是否用原生内嵌 Webview 呈现（仅 DSH 界面为 true）。 */
  native: boolean;
}

/** 内嵌 Webview 位置/尺寸（逻辑像素，相对主窗口客户区；由 getBoundingClientRect() 得出）。 */
export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
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

/** 单个插件的更新检测结果。 */
export interface PluginUpdateInfo {
  key: string;
  currentVersion: string | null;
  latestVersion: string | null;
  updateAvailable: boolean;
  /** 来源：npm / github / unknown。 */
  source: string;
  /** 检测失败原因（网络查询失败 / 已装版本无法读取）；null 表示检测正常完成。 */
  error: string | null;
}

/** 指定 profile 的插件更新检测结果。 */
export interface PluginUpdates {
  profile: string;
  checkedAt: string;
  entries: PluginUpdateInfo[];
}

/** 余额查询结果（与 Rust balance.rs 的 BalanceResult 对齐）。 */
export interface BalanceResult {
  available: boolean;
  apiKeySet: boolean;
  isAvailable: boolean | null;
  currency: string | null;
  balance: number | null;
  error: string | null;
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

/** 格式化货币金额（CNY 显示 ¥，其余带币种前缀）。 */
export function formatMoney(value: number, currency: string): string {
  const sym = currency === "CNY" ? "¥" : `${currency} `;
  return `${sym}${value.toFixed(2)}`;
}

/** 将 Date 格式化为 `YYYY-MM-DD HH:mm:ss`（与后端 now_string() 输出一致，用于日志时间戳）。 */
export function formatDateTime(d: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}
