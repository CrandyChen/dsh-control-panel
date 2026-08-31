import {
  App as AntApp,
  ConfigProvider,
  Layout,
  Progress,
  Space,
  Typography,
  theme,
} from "antd";
import enUS from "antd/locale/en_US";
import zhCN from "antd/locale/zh_CN";
import { getCurrentWindow } from "@tauri-apps/api/window";
import dayjs from "dayjs";
import "dayjs/locale/zh-cn";
import "dayjs/locale/en";
import { useCallback, useEffect, useState } from "react";
import { api } from "./api";
import ActionBar from "./components/ActionBar";
import InstallModal from "./components/InstallModal";
import LogPanel from "./components/LogPanel";
import PluginManagerDialog from "./components/PluginManagerDialog";
import SettingsDrawer from "./components/SettingsDrawer";
import StatusCard from "./components/StatusCard";
import TabFrame from "./components/TabFrame";
import UninstallDialog from "./components/UninstallDialog";
import WebTabBar from "./components/WebTabBar";
import { I18nContext, detectSystemLang, makeT, useI18n } from "./i18n";
import type { Lang } from "./i18n";
import { buildInstallGuideHtml } from "./installGuide";
import type { ToolStatus } from "./types";
import { formatSize } from "./types";
import { usePanel } from "./usePanel";

export default function App() {
  const [dark, setDark] = useState(true);
  const [lang, setLang] = useState<Lang>(detectSystemLang());
  return (
    <ConfigProvider
      locale={lang === "zh-CN" ? zhCN : enUS}
      theme={{
        algorithm: dark ? theme.darkAlgorithm : theme.defaultAlgorithm,
        token: { borderRadius: 8 },
      }}
    >
      <AntApp>
        <I18nContext.Provider value={{ lang, t: makeT(lang) }}>
          <Shell onTheme={setDark} lang={lang} onLang={setLang} />
        </I18nContext.Provider>
      </AntApp>
    </ConfigProvider>
  );
}

function Shell({
  onTheme,
  lang,
  onLang,
}: {
  onTheme: (d: boolean) => void;
  lang: Lang;
  onLang: (l: Lang) => void;
}) {
  const { message } = AntApp.useApp();
  const { t } = useI18n();
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [installOpen, setInstallOpen] = useState(false);
  const [uninstallOpen, setUninstallOpen] = useState(false);
  const [pluginsOpen, setPluginsOpen] = useState(false);

  const l = usePanel();
  const busy = l.phase !== "idle";
  const running = l.webStatus === "ready" || l.webStatus === "starting";
  const hasTabs = l.tabs.length > 0;
  const homeVisible = l.activeTabId === null;

  // 生效语言：配置非 auto 时以配置为准（auto 跟随系统，非中英默认英文）。
  useEffect(() => {
    const cfgLang = l.config?.language;
    const effective = cfgLang && cfgLang !== "auto" ? cfgLang : detectSystemLang();
    if (effective !== lang) onLang(effective);
  }, [l.config?.language, lang, onLang]);

  // 主题：按配置（auto/light/dark）解析并应用。auto 跟随系统偏好。
  useEffect(() => {
    const setting = l.config?.theme ?? "auto";
    const d =
      setting === "dark"
        ? true
        : setting === "light"
          ? false
          : (window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false);
    onTheme(d);
  }, [l.config?.theme, onTheme]);

  // auto 模式：监听系统主题变化，实时切换深浅。
  useEffect(() => {
    if ((l.config?.theme ?? "auto") !== "auto") return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const handler = (e: MediaQueryListEvent) => onTheme(e.matches);
    mq.addEventListener?.("change", handler);
    return () => mq.removeEventListener?.("change", handler);
  }, [l.config?.theme, onTheme]);

  // dayjs / 文档标题 / 窗口标题随语言切换。
  useEffect(() => {
    dayjs.locale(lang === "zh-CN" ? "zh-cn" : "en");
    const title =
      lang === "zh-CN"
        ? "DSH Control Panel · DeepSeek Harness 控制面板"
        : "DSH Control Panel · DeepSeek Harness Control Panel";
    document.title = title;
    try {
      void getCurrentWindow().setTitle(title);
    } catch {
      /* 窗口标题设置失败不影响使用 */
    }
  }, [lang]);

  /** 打开安装指引 tab（动态生成的自包含 HTML，blob URL 形式）。 */
  const openGuideTab = useCallback(
    (tools: ToolStatus[]) => {
      const url = URL.createObjectURL(
        new Blob([buildInstallGuideHtml(tools, lang)], { type: "text/html" }),
      );
      l.openTab(url, t("msg.openGuide"));
    },
    [l.openTab, lang, t],
  );

  /** 打开安装指引 tab：点击时拉取一次新鲜环境检测，保证指引内容与实际一致。 */
  const handleOpenGuide = useCallback(async () => {
    let tools: ToolStatus[] = l.tools ?? [];
    try {
      tools = await api.detectTools();
    } catch {
      // 检测失败时沿用缓存结果。
    }
    openGuideTab(tools);
  }, [l.tools, openGuideTab]);

  /** 点击「安装」：源码模式先做环境检测，git 缺失则打开指引 tab；预构建模式直接安装。 */
  const onInstallClick = useCallback(async () => {
    const mode = l.config?.installMode ?? "prebuilt";
    if (mode === "source") {
      let tools: ToolStatus[] = l.tools ?? [];
      try {
        tools = await api.detectTools();
      } catch {
        // 检测失败时沿用缓存结果，不阻塞主流程。
      }
      const blocked = tools.some((t) => t.required && (!t.installed || !t.ok));
      if (blocked) {
        message.warning(t("msg.envBlocked"));
        openGuideTab(tools);
        return;
      }
    }
    setInstallOpen(true);
  }, [l.config?.installMode, l.tools, message, openGuideTab, t]);

  /** 「打开界面」：按配置的默认方式分发（tab = 程序内新标签页，browser = 系统浏览器）。 */
  const handleOpenUi = useCallback(() => {
    if (l.config?.openUiMode === "browser") {
      void l.openWebUi();
    } else {
      l.openDshTab();
    }
  }, [l.config?.openUiMode, l.openWebUi, l.openDshTab]);

  /** 接收安装指引页发来的外链请求，交给 opener 插件在系统浏览器打开。 */
  useEffect(() => {
    const handler = (e: MessageEvent) => {
      const data = e.data as { type?: string; url?: unknown } | null;
      if (!data || data.type !== "dsh-open-external") return;
      const u = data.url;
      if (typeof u !== "string") return;
      try {
        const parsed = new URL(u);
        if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return;
        void api.openExternal(u);
      } catch {
        /* 非法 URL 忽略 */
      }
    };
    window.addEventListener("message", handler);
    return () => window.removeEventListener("message", handler);
  }, []);

  const homeHint = hasTabs
    ? t("home.hint.tabs")
    : running
      ? l.config?.openUiMode === "browser"
        ? t("home.hint.running.browser")
        : t("home.hint.running.tab")
      : l.detect?.installed
        ? t("home.hint.installed")
        : t("home.hint.notInstalled");

  return (
    <Layout style={{ height: "100vh", overflow: "hidden" }}>
      {hasTabs && (
        <WebTabBar
          tabs={l.tabs}
          activeTabId={l.activeTabId}
          onSelect={l.focusTab}
          onClose={l.closeTab}
          onNew={l.openTab}
          onShowHome={l.showHome}
        />
      )}

      {/* 内容容器：控制面板主界面常驻挂载（display 切换），各 Tab 为 iframe */}
      <Layout.Content style={{ position: "relative", overflow: "hidden", flex: 1 }}>
        {/* 控制面板主界面（永不销毁，activeTabId===null 时显示） */}
        <div
          style={{
            position: "absolute",
            inset: 0,
            overflowY: "auto",
            display: homeVisible ? "block" : "none",
          }}
        >
          <div
            style={{
              padding: 24,
              display: "flex",
              flexDirection: "column",
              gap: 16,
              width: "100%",
            }}
          >
            <StatusCard
              config={l.config}
              detect={l.detect}
              webStatus={l.webStatus}
              lastCheck={l.lastCheck}
              busy={busy}
              balance={l.balance}
              onOpenSettings={() => setSettingsOpen(true)}
            />

            <ActionBar
              config={l.config}
              detect={l.detect}
              phase={l.phase}
              webStatus={l.webStatus}
              onInstallClick={onInstallClick}
              onStart={l.start}
              onOpenUi={handleOpenUi}
              onStop={l.stop}
              onUpdate={l.update}
              onCheck={l.checkForUpdates}
              onTerminal={l.openTerminal}
              onUninstall={() => {
                // 立即打开对话框并后台统计卸载清单（统计耗时较长，对话框内展示进度）。
                setUninstallOpen(true);
                void l.loadPreview();
              }}
              onPlugins={() => setPluginsOpen(true)}
              pluginUpdates={l.pluginUpdates}
            />

            {/* 预构建内核下载 / 解压进度（安装 / 更新 / 修复预构建模式时实时显示） */}
            {l.progress && (
              <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
                <Progress
                  size="small"
                  status="active"
                  percent={
                    l.progress.total > 0
                      ? Math.min(100, Math.round((l.progress.received / l.progress.total) * 100))
                      : undefined
                  }
                />
                <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                  {(() => {
                    const p = l.progress;
                    const key =
                      p.step === "extract"
                        ? p.total > 0
                          ? "extract.progress"
                          : "extract.progress.unknown"
                        : p.step === "clone"
                          ? p.total > 0
                            ? "clone.progress"
                            : "clone.progress.unknown"
                          : p.total > 0
                            ? "download.progress"
                            : "download.progress.unknown";
                    return p.total > 0
                      ? t(key, {
                          0: formatSize(p.received),
                          1: formatSize(p.total),
                          2: Math.round((p.received / p.total) * 100),
                        })
                      : t(key, {
                          0: formatSize(p.received),
                        });
                  })()}
                </Typography.Text>
              </div>
            )}

            {/* 运行环境（node/pnpm）下载/解压进度：与内核下载并行时的次要提示 */}
            {l.runtimeProgress && (
              <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
                <Progress
                  size="small"
                  status="active"
                  percent={
                    l.runtimeProgress.total > 0
                      ? Math.min(100, Math.round((l.runtimeProgress.received / l.runtimeProgress.total) * 100))
                      : undefined
                  }
                />
                <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                  {l.runtimeProgress.step === "runtime-ex"
                    ? l.runtimeProgress.total > 0
                      ? t("runtime.extract", {
                          0: formatSize(l.runtimeProgress.received),
                          1: formatSize(l.runtimeProgress.total),
                        })
                      : t("runtime.extract.unknown", { 0: formatSize(l.runtimeProgress.received) })
                    : l.runtimeProgress.total > 0
                      ? t("runtime.progress", {
                          0: formatSize(l.runtimeProgress.received),
                          1: formatSize(l.runtimeProgress.total),
                        })
                      : t("runtime.progress.unknown", { 0: formatSize(l.runtimeProgress.received) })}
                </Typography.Text>
              </div>
            )}

            <Space direction="vertical" size={4} style={{ width: "100%" }}>
              <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                {homeHint}
              </Typography.Text>
              <LogPanel
                logs={l.logs}
                busy={busy}
                currentStep={l.currentStep}
                logDir={l.logDir}
                onClear={l.clearLogs}
              />
            </Space>
          </div>
        </div>

        {/* 各 Tab 页面（iframe 常驻挂载，display 切换，保留页面状态） */}
        {l.tabs.map((tab) => (
          <TabFrame key={tab.id} tab={tab} active={l.activeTabId === tab.id} />
        ))}
      </Layout.Content>

      <InstallModal
        open={installOpen}
        tools={l.tools}
        installMode={l.config?.installMode ?? "prebuilt"}
        installedKernels={l.config?.installedKernels ?? []}
        onOpenGuide={handleOpenGuide}
        onCancel={() => setInstallOpen(false)}
        onInstall={async (mode, version) => {
          // 点击「安装 / 修复安装」后立即关闭对话框：进度与报错在日志面板实时展示。
          setInstallOpen(false);
          void l.install(mode, version);
        }}
        onRepair={async (kernelId) => {
          setInstallOpen(false);
          void l.repair(kernelId);
        }}
      />

      <UninstallDialog
        preview={l.preview}
        open={uninstallOpen}
        busy={l.phase === "uninstalling"}
        loading={l.previewLoading}
        progress={l.previewProgress}
        onCancel={() => {
          // 统计进行中关闭对话框时取消后台扫描，避免空转。
          if (l.previewLoading) void l.cancelPreview();
          setUninstallOpen(false);
        }}
        onCancelScan={() => {
          void l.cancelPreview();
          setUninstallOpen(false);
        }}
        onConfirm={async (sel) => {
          await l.uninstall(sel);
          setUninstallOpen(false);
        }}
      />

      <SettingsDrawer
        open={settingsOpen}
        config={l.config}
        onClose={() => setSettingsOpen(false)}
        onSave={l.saveSettings}
      />

      <PluginManagerDialog
        open={pluginsOpen}
        config={l.config}
        webRunning={running}
        onClose={() => setPluginsOpen(false)}
        onSaveSettings={l.saveSettings}
      />
    </Layout>
  );
}
