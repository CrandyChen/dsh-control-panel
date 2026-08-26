// 控制面板核心状态钩子：管理配置、探测结果、阶段、日志、内嵌浏览器 Tab 与所有动作。

import { App } from "antd";
import { createElement, useCallback, useEffect, useRef, useState } from "react";
import { api, onLogLine, onPluginUpdatesChecked, onUpdateChecked, onWebStatus } from "./api";
import { WEB_URL } from "./constants";
import { useI18n } from "./i18n";
import { formatMoney } from "./types";
import type {
  BalanceResult,
  BrowserTab,
  DetectResult,
  AppConfig,
  LogLine,
  Phase,
  PipelineEvent,
  PluginUpdates,
  ToolStatus,
  UninstallPreview,
  UpdateCheckResult,
  WebStatus,
} from "./types";

const MAX_LOG = 600;

/** 从 URL 提取标题（host，失败用默认名）。 */
function titleFromUrl(url: string, fallback: string): string {
  try {
    return new URL(url).host || fallback;
  } catch {
    return fallback;
  }
}

export interface Panel {
  config: AppConfig | null;
  detect: DetectResult | null;
  /** 运行环境工具检测结果（仅源码模式 Git；预构建模式为空数组）。 */
  tools: ToolStatus[] | null;
  phase: Phase;
  logs: LogLine[];
  webStatus: WebStatus;
  lastCheck: UpdateCheckResult | null;
  preview: UninstallPreview | null;
  currentStep: string | null;
  /** 预构建内核下载/解压进度（step="download"|"extract"；received/total 字节、speedBps 字节/秒）；非空表示进行中。 */
  progress: { step: string; received: number; total: number; speedBps: number } | null;
  /** 运行环境（node/pnpm）下载进度（step="runtime"）；非空表示进行中。 */
  runtimeProgress: { step: string; received: number; total: number; speedBps: number } | null;
  /** 当前余额（安装 DSH 且配置 API 后每 5 分钟查询一次）。 */
  balance: BalanceResult | null;
  /** 手动触发一次余额查询。 */
  checkBalance: () => Promise<void>;
  /** 内嵌浏览器 Tab（纯前端状态）；activeTabId 为 null 时显示控制面板主界面。 */
  tabs: BrowserTab[];
  activeTabId: string | null;
  manualCandidates: string[] | null;
  logDir: string;
  refresh: () => Promise<void>;
  /** 重新检测运行环境工具（仅源码模式 Git；预构建模式返回空）并更新 tools 状态。 */
  refreshTools: () => Promise<ToolStatus[]>;
  install: (dir: string, mode: string) => Promise<void>;
  /** 检测新版本：返回结果（失败为 null），由调用方决定是否展示更新详情对话框。 */
  checkForUpdates: () => Promise<UpdateCheckResult | null>;
  update: () => Promise<void>;
  /** 修复安装：清理异常状态并分级重建（详见后端 repair.rs）。 */
  repair: () => Promise<void>;
  /** 卸载预览是否正在统计目录大小/文件数（生成清单）。 */
  previewLoading: boolean;
  /** 卸载预览扫描的最新进度行（后端推送，step="scan"）。 */
  previewProgress: string | null;
  loadPreview: () => Promise<void>;
  /** 取消进行中的卸载预览扫描。 */
  cancelPreview: () => Promise<void>;
  uninstall: (selected: string[]) => Promise<void>;
  start: () => Promise<void>;
  stop: () => Promise<void>;
  openTerminal: () => Promise<void>;
  openWebUi: () => Promise<void>;
  openDshTab: () => void;
  openTab: (url: string, title?: string) => void;
  closeTab: (id: string) => void;
  focusTab: (id: string) => void;
  showHome: () => void;
  adoptInstall: (path: string) => Promise<void>;
  ignoreManualInstall: () => void;
  saveSettings: (patch: Partial<AppConfig>) => Promise<void>;
  clearLogs: () => Promise<void>;
  appendLog: (level: string, text: string) => void;
  /** 最近一次插件更新检测结果（默认 profile）。 */
  pluginUpdates: PluginUpdates | null;
  /** 手动触发一次当前 profile 的插件更新检测。 */
  checkPluginUpdates: () => Promise<void>;
}

export function usePanel(): Panel {
  const { message, modal } = App.useApp();
  const { t } = useI18n();
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [detect, setDetect] = useState<DetectResult | null>(null);
  const [tools, setTools] = useState<ToolStatus[] | null>(null);
  const [phase, setPhase] = useState<Phase>("idle");
  const [logs, setLogs] = useState<LogLine[]>([]);
  const [webStatus, setWebStatus] = useState<WebStatus>("idle");
  const [lastCheck, setLastCheck] = useState<UpdateCheckResult | null>(null);
  const [preview, setPreview] = useState<UninstallPreview | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [previewProgress, setPreviewProgress] = useState<string | null>(null);
  const [currentStep, setCurrentStep] = useState<string | null>(null);
  /** 预构建内核下载/解压进度（step="download"|"extract"；非空表示进行中）。 */
  const [progress, setProgress] = useState<{
    step: string;
    received: number;
    total: number;
    speedBps: number;
  } | null>(null);
  /** 运行环境（node/pnpm）下载/解压进度（step="runtime"=下载，"runtime-ex"=解压；非空表示进行中）。 */
  const [runtimeProgress, setRuntimeProgress] = useState<{
    step: string;
    received: number;
    total: number;
    speedBps: number;
  } | null>(null);
  const [balance, setBalance] = useState<BalanceResult | null>(null);
  const [tabs, setTabs] = useState<BrowserTab[]>([]);
  const [activeTabId, setActiveTabId] = useState<string | null>(null);
  const [manualCandidates, setManualCandidates] = useState<string[] | null>(null);
  /** 最近一次插件更新检测结果（默认为当前插件 profile；周期/手动触发更新）。 */
  const [pluginUpdates, setPluginUpdates] = useState<PluginUpdates | null>(null);
  const [logDir, setLogDir] = useState("");

  const manualIgnored = useRef(false);
  const manualScanned = useRef(false);
  const startedByUs = useRef(false);
  /** 启动失败后是否已提示过修复安装（每会话一次，避免打扰）。 */
  const repairSuggested = useRef(false);
  /** 用户是否请求取消本次卸载预览扫描（取消后扫描返回的错误静默处理）。 */
  const previewCancelled = useRef(false);
  /** 低余额提醒触发标记（每会话每阈值一次；回升后再跌穿才再次提醒）。 */
  const alerted10 = useRef(false);
  const alerted5 = useRef(false);
  /** 最新配置快照（供事件回调读取 openUiMode 等设置，避免订阅反复重建）。 */
  const configRef = useRef(config);
  configRef.current = config;

  const appendLog = useCallback((level: string, text: string) => {
    setLogs((prev) => {
      const next = [...prev, { level, text, ts: new Date().toLocaleTimeString() }];
      return next.length > MAX_LOG ? next.slice(next.length - MAX_LOG) : next;
    });
  }, []);

  const refresh = useCallback(async () => {
    try {
      const [cfg, det] = await Promise.all([api.getConfig(), api.detectState()]);
      setConfig(cfg);
      setDetect(det);
      // 启动流程进行中（starting）时不要覆盖状态，避免按钮提前恢复。
      setWebStatus((prev) => (prev === "starting" ? prev : det.running ? "ready" : "idle"));
      // 未配置安装目录时，扫描一次本机手动安装的 DeepSeek Harness（不重复扫描）。
      if (!cfg.installDir && !manualIgnored.current && !manualScanned.current) {
        manualScanned.current = true;
        const found = await api.scanManualInstalls();
        if (found.length > 0) {
          setManualCandidates(found);
        }
      }
    } catch (e) {
      message.error(t("msg.refreshFail", { 0: String(e) }));
    }
  }, [message, t]);

  useEffect(() => {
    void refresh();
    // 环境检测仅启动时拉取一次（工具只在外部安装/卸载时变化）；
    // 点击「安装」/「查看安装指引」时会另行获取新鲜结果。
    void api.detectTools().then(setTools).catch(() => undefined);
    void api.getLogDir().then(setLogDir);
    // 日志只展示本次会话：不加载历史文件，仅接收实时事件。
    const unsubs = [
      onLogLine((l) => setLogs((prev) => [...prev.slice(-(MAX_LOG - 1)), l])),
      onWebStatus((s) => {
        // 运行状态由事件直接驱动（webStatus 已更新，无需再全量刷新）。
        setWebStatus(s);
        if (s === "ready") {
          // 仅当由本程序启动成功时自动打开界面：按「打开界面默认方式」设置分发
          // （browser = 系统浏览器；tab（默认）= 程序内新标签页）。
          if (startedByUs.current) {
            startedByUs.current = false;
            if (configRef.current?.openUiMode === "browser") {
              void openWebUiRef.current();
            } else {
              void openDshTabRef.current();
            }
          }
        }
        // 启动尝试后服务异常退出（error / 非用户主动停止的 stopped）→ 友好建议修复安装。
        if ((s === "error" || s === "stopped") && startedByUs.current) {
          startedByUs.current = false;
          if (!repairSuggested.current) {
            repairSuggested.current = true;
            modal.confirm({
              title: t("sug.title"),
              content: createElement(
                "div",
                { style: { fontSize: 13, lineHeight: 1.9 } },
                t("sug.desc1"),
                createElement("br"),
                t("sug.desc2"),
                createElement("br"),
                createElement("br"),
                t("sug.risk"),
                createElement("br"),
                t("sug.risk1"),
                createElement("br"),
                t("sug.risk2"),
                createElement("br"),
                t("sug.risk3"),
              ),
              okText: t("sug.ok"),
              cancelText: t("sug.cancel"),
              onOk: () => {
                void repairRef.current();
              },
            });
          }
        }
      }),
      onUpdateChecked((r) => {
        setLastCheck(r);
        // 检测结果已随事件返回，直接在本地更新配置，避免触发全量刷新。
        setConfig((prev) =>
          prev
            ? {
                ...prev,
                updateAvailable: r.updateAvailable,
                latestCommit: r.remoteCommit,
                latestSubject: r.subject,
                lastCheckAt: r.checkedAt,
              }
            : prev,
        );
      }),
      onPluginUpdatesChecked((u) => setPluginUpdates(u)),
    ];
    return () => {
      for (const u of unsubs) void u.then((f) => f());
    };
  }, [refresh, t]);

  /** 长任务通用包装：设置阶段、转发事件、报错提示（网络不可达走对话框）。 */
  const withPhase = useCallback(
    async (p: Phase, run: () => Promise<void>, okText: string) => {
      setPhase(p);
      setCurrentStep(null);
      try {
        await run();
        message.success(okText);
      } catch (e) {
        const msg = String(e);
        if (msg.startsWith("网络不可达") || msg.startsWith("Network unreachable")) {
          modal.error({
            title: t("msg.network.title"),
            content: createElement(
              "div",
              { style: { fontSize: 13, lineHeight: 1.8 } },
              msg,
              createElement("br"),
              t("msg.network.desc"),
            ),
            okText: t("msg.network.ok"),
          });
        } else {
          message.error(msg, 6);
        }
      } finally {
        setPhase("idle");
        setCurrentStep(null);
        setProgress(null);
        setRuntimeProgress(null);
        void refresh();
      }
    },
    [message, modal, refresh, t],
  );

  const onPipelineEvent = useCallback(
    (e: PipelineEvent) => {
      // 步骤标题本地化：优先按 step id 映射（如 clone/install/build/repair-*），未知 id 回退原文。
      const stepTitle = (id: string, fallback: string): string => {
        const localized = t(`step.${id}`);
        return localized !== `step.${id}` ? localized : fallback;
      };
      switch (e.type) {
        case "stepStarted": {
          const title = stepTitle(e.id, e.title);
          setCurrentStep(title);
          setProgress(null);
          // 运行环境与「clone」并行下载；进入其它步骤（web/install/extract）时运行环境已就绪，隐藏其进度。
          if (e.id !== "clone") setRuntimeProgress(null);
          appendLog("INFO", `▶ ${title}`);
          break;
        }
        case "output":
          appendLog(e.stream === "stderr" ? "WARN" : "INFO", e.line);
          break;
        case "stepFinished": {
          const title = stepTitle(e.id, e.id);
          setProgress(null);
          setRuntimeProgress(null);
          appendLog("INFO", `✓ ${title} (exit ${e.exitCode})`);
          break;
        }
        case "downloadProgress":
          if (e.step === "runtime" || e.step === "runtime-ex") {
            setRuntimeProgress({
              step: e.step,
              received: e.received,
              total: e.total,
              speedBps: e.speedBps,
            });
          } else {
            // 主进度（内核下载/解压/克隆）；不覆盖并行进行的运行环境进度。
            setProgress({ step: e.step, received: e.received, total: e.total, speedBps: e.speedBps });
          }
          break;
        case "runtimeDone":
          // 运行环境（node/pnpm）流程结束：隐藏运行环境进度条。
          setRuntimeProgress(null);
          break;
        case "error":
          setProgress(null);
          setRuntimeProgress(null);
          appendLog("ERROR", e.message);
          break;
        case "finished":
          setProgress(null);
          setRuntimeProgress(null);
          break;
      }
    },
    [appendLog, t],
  );

  const refreshTools = useCallback(async (): Promise<ToolStatus[]> => {
    const tools = await api.detectTools();
    setTools(tools);
    return tools;
  }, []);

  /** 查询当前余额并处理低余额提醒（每会话每阈值一次；回升后再跌穿才再次提醒）。 */
  const checkBalance = useCallback(async () => {
    try {
      const r = await api.getBalance();
      setBalance(r);
      const b = r.balance;
      if (b == null) return;
      if (b >= 10) alerted10.current = false;
      if (b >= 5) alerted5.current = false;
      if (b < 10 && b >= 5 && !alerted10.current) {
        alerted10.current = true;
        message.warning(t("msg.balance.low10", { 0: formatMoney(b, r.currency ?? "CNY") }), 6);
      }
      if (b < 5 && !alerted5.current) {
        alerted5.current = true;
        message.error(t("msg.balance.low5", { 0: formatMoney(b, r.currency ?? "CNY") }), 6);
      }
    } catch {
      // 余额查询失败静默（卡片显示「—」）。
    }
  }, [message, t]);

  // 安装 DSH 后每 5 分钟查询一次余额。
  useEffect(() => {
    if (!detect?.installed) {
      setBalance(null);
      return;
    }
    void checkBalance();
    const timer = window.setInterval(() => void checkBalance(), 5 * 60 * 1000);
    return () => window.clearInterval(timer);
  }, [detect?.installed, checkBalance]);

  const install = useCallback(
    (dir: string, mode: string) =>
      withPhase("installing", () => api.install(dir, mode, onPipelineEvent), t("msg.installDone")),
    [onPipelineEvent, withPhase, t],
  );

  /** 检测新版本：返回检测结果（失败或网络不可达返回 null，由调用方决定 UI）。
   * 合并后的「更新」按钮流程：先调用本方法，有新版时展示详情对话框。 */
  const checkForUpdates = useCallback(async (): Promise<UpdateCheckResult | null> => {
    setPhase("checking");
    try {
      const r = await api.checkForUpdates();
      setLastCheck(r);
      return r;
    } catch (e) {
      const msg = String(e);
      if (msg.startsWith("网络不可达") || msg.startsWith("Network unreachable")) {
        modal.error({
          title: t("msg.network.title"),
          content: createElement(
            "div",
            { style: { fontSize: 13, lineHeight: 1.8 } },
            msg,
            createElement("br"),
            t("msg.network.desc"),
          ),
          okText: t("msg.network.ok"),
        });
      } else {
        message.error(msg, 6);
      }
      return null;
    } finally {
      setPhase("idle");
      void refresh();
    }
  }, [message, modal, refresh, t]);

  const update = useCallback(
    () => withPhase("updating", () => api.update(onPipelineEvent), t("msg.updateDone")),
    [onPipelineEvent, withPhase, t],
  );

  /** 修复安装：清理异常状态并分级重建（详见后端 repair.rs）。 */
  const repair = useCallback(
    () => withPhase("repairing", () => api.repairInstall(onPipelineEvent), t("msg.repairDone")),
    [onPipelineEvent, withPhase, t],
  );
  const repairRef = useRef(repair);
  repairRef.current = repair;

  /** 生成卸载预览清单：统计安装目录与 ~/.dsh 大小/文件数，期间推送实时进度。
   * 扫描可能耗时较长（node_modules 数万文件），由调用方在对话框内展示 loading。 */
  const loadPreview = useCallback(async () => {
    previewCancelled.current = false;
    setPhase("previewing");
    setPreviewLoading(true);
    setPreviewProgress(null);
    setPreview(null);
    try {
      const p = await api.uninstallPreview((e) => {
        // 后端进度行（step="scan"）仅用于对话框展示，不写入主日志。
        if (e.type === "output" && e.step === "scan") setPreviewProgress(e.line);
      });
      setPreview(p);
    } catch (e) {
      if (!previewCancelled.current) {
        message.error(t("msg.previewFail", { 0: String(e) }));
      }
    } finally {
      setPhase("idle");
      setPreviewLoading(false);
    }
  }, [message, t]);

  /** 取消进行中的卸载预览扫描（无扫描时为空操作）。 */
  const cancelPreview = useCallback(async () => {
    previewCancelled.current = true;
    try {
      await api.cancelUninstallPreview();
    } catch {
      // 取消失败可忽略：扫描会自然结束。
    }
  }, []);

  const uninstall = useCallback(
    (selected: string[]) =>
      withPhase("uninstalling", () => api.uninstall(selected, onPipelineEvent), t("msg.uninstallDone")),
    [onPipelineEvent, withPhase, t],
  );

  const start = useCallback(() => {
    startedByUs.current = true;
    appendLog("INFO", t("msg.startLog"));
    return withPhase(
      "starting",
      () => api.startWeb(onPipelineEvent),
      t("msg.startHint"),
    );
  }, [onPipelineEvent, withPhase, appendLog, t]);

  const stop = useCallback(
    () =>
      withPhase("stopping", async () => {
        await api.stopWeb();
      }, t("msg.stopped")),
    [withPhase, t],
  );

  const openTerminal = useCallback(async () => {
    try {
      await api.openTerminal();
    } catch (e) {
      message.error(String(e));
    }
  }, [message]);

  const openWebUi = useCallback(async () => {
    try {
      await api.openWebUi();
    } catch (e) {
      message.error(t("msg.openUiFail", { 0: String(e) }));
    }
  }, [message, t]);

  const openWebUiRef = useRef(openWebUi);
  openWebUiRef.current = openWebUi;

  /** 打开 DeepSeek Harness 界面：已有 DSH Tab 则激活，否则新建。 */
  const openDshTab = useCallback(() => {
    const existing = tabs.find((t) => t.url === WEB_URL);
    if (existing) {
      setActiveTabId(existing.id);
    } else {
      openTabRef.current(WEB_URL);
    }
  }, [tabs]);

  const openDshTabRef = useRef(openDshTab);
  openDshTabRef.current = openDshTab;

  /** 新建 Tab（本地状态）。title 可覆盖（如 blob 指引页无法从 URL 推导标题）。 */
  const openTab = useCallback(
    (url: string, title?: string) => {
      const id = `tab-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
      setTabs((prev) => [...prev, { id, title: title ?? titleFromUrl(url, t("tab.new")), url }]);
      setActiveTabId(id);
    },
    [t],
  );

  const openTabRef = useRef(openTab);
  openTabRef.current = openTab;

  /** 关闭 Tab；关闭激活项则回到控制面板主界面。 */
  const closeTab = useCallback((id: string) => {
    setTabs((prev) => {
      const next = prev.filter((t) => t.id !== id);
      if (next.length === 0) setActiveTabId(null);
      return next;
    });
    setActiveTabId((prev) => (prev === id ? null : prev));
  }, []);

  const focusTab = useCallback((id: string) => {
    setActiveTabId(id);
  }, []);

  /** 回到控制面板主界面（默认界面常驻，不销毁）。 */
  const showHome = useCallback(() => {
    setActiveTabId(null);
  }, []);

  /** 采用手动安装的 DSH 目录，随后尝试检测一次更新（失败静默）。 */
  const adoptInstall = useCallback(
    async (path: string) => {
      try {
        await api.adoptInstall(path);
        setManualCandidates(null);
        message.success(t("msg.adoptDone", { 0: path }));
        await refresh();
        try {
          const r = await api.checkForUpdates();
          setLastCheck(r);
          if (r.updateAvailable) {
            message.warning(t("msg.newVersionShort", { 0: r.behind }), 5);
          }
        } catch {
          message.info(t("msg.checkFail"), 4);
        }
      } catch (e) {
        message.error(String(e));
      }
    },
    [message, refresh, t],
  );

  const ignoreManualInstall = useCallback(() => {
    manualIgnored.current = true;
    setManualCandidates(null);
  }, []);

  const saveSettings = useCallback(
    async (patch: Partial<AppConfig>) => {
      if (!config) return;
      const next = { ...config, ...patch };
      try {
        await api.saveConfig(next);
        setConfig(next);
        message.success(t("msg.settingsSaved"));
      } catch (e) {
        message.error(t("msg.settingsFail", { 0: String(e) }));
      }
    },
    [config, message, t],
  );

  const clearLogs = useCallback(async () => {
    try {
      await api.clearLogs();
      setLogs([]);
    } catch (e) {
      message.error(t("msg.clearLogsFail", { 0: String(e) }));
    }
  }, [message, t]);

  /** 手动触发当前 profile 的插件更新检测（结果写入 pluginUpdates，供徽标/列表展示）。 */
  const checkPluginUpdates = useCallback(async () => {
    const p = config?.pluginProfile || "web";
    try {
      const u = await api.pluginCheckUpdates(p);
      setPluginUpdates(u);
    } catch (e) {
      message.error(t("plugin.updates.fail", { 0: String(e) }), 5);
    }
  }, [config?.pluginProfile, message, t]);

  return {
    config,
    detect,
    tools,
    phase,
    logs,
    webStatus,
    lastCheck,
    preview,
    currentStep,
    progress,
    runtimeProgress,
    balance,
    checkBalance,
    tabs,
    activeTabId,
    manualCandidates,
    logDir,
    refresh,
    refreshTools,
    install,
    checkForUpdates,
    update,
    repair,
    previewLoading,
    previewProgress,
    loadPreview,
    cancelPreview,
    uninstall,
    start,
    stop,
    openTerminal,
    openWebUi,
    openDshTab,
    openTab,
    closeTab,
    focusTab,
    showHome,
    adoptInstall,
    ignoreManualInstall,
    saveSettings,
    clearLogs,
    appendLog,
    pluginUpdates,
    checkPluginUpdates,
  };
}
