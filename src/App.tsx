import { SettingOutlined, ThunderboltOutlined } from "@ant-design/icons";
import {
  App as AntApp,
  Badge,
  Button,
  ConfigProvider,
  Flex,
  Layout,
  Space,
  Tag,
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
import ManualInstallModal from "./components/ManualInstallModal";
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
          <Shell dark={dark} onTheme={setDark} lang={lang} onLang={setLang} />
        </I18nContext.Provider>
      </AntApp>
    </ConfigProvider>
  );
}

function Shell({
  dark,
  onTheme,
  lang,
  onLang,
}: {
  dark: boolean;
  onTheme: (d: boolean) => void;
  lang: Lang;
  onLang: (l: Lang) => void;
}) {
  const { token } = theme.useToken();
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

  /** 点击「安装」：先做一次新鲜的环境检测；必装项缺失则打开指引 tab 而非安装弹窗。 */
  const onInstallClick = useCallback(async () => {
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
    setInstallOpen(true);
  }, [l.tools, message, openGuideTab, t]);

  /** 「检测运行环境」：拉取新鲜检测结果，刷新标签并给出摘要提示。 */
  const handleDetectTools = useCallback(async () => {
    try {
      const tools = await l.refreshTools();
      const missing = tools.filter((t) => !t.installed || !t.ok);
      if (missing.length === 0) {
        message.success(t("status.detect.ok"));
      } else {
        message.warning(
          t("status.detect.issues", {
            0: missing.length,
            1: missing.map((t) => t.name).join("、"),
          }),
          5,
        );
      }
    } catch (e) {
      message.error(t("status.detect.fail", { 0: String(e) }));
    }
  }, [l.refreshTools, message, t]);

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
      <Layout.Header
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          paddingInline: 24,
          borderBottom: `1px solid ${token.colorBorderSecondary}`,
          flexShrink: 0,
        }}
      >
        <Flex align="center" gap={12}>
          <ThunderboltOutlined style={{ fontSize: 22, color: token.colorPrimary }} />
          <Typography.Title level={4} style={{ margin: 0, whiteSpace: "nowrap" }}>
            DSH Control Panel
          </Typography.Title>
          <Tag style={{ marginInlineEnd: 0 }}>{t("app.tag")}</Tag>
          {l.config?.updateAvailable && (
            <Badge
              status="error"
              text={
                <span style={{ color: token.colorError, fontSize: 13 }}>
                  {t("app.newVersion")}
                </span>
              }
            />
          )}
        </Flex>
        <Button icon={<SettingOutlined />} onClick={() => setSettingsOpen(true)}>
          {t("app.settings")}
        </Button>
      </Layout.Header>

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
              maxWidth: 1200,
              width: "100%",
              margin: "0 auto",
            }}
          >
            <StatusCard
              config={l.config}
              detect={l.detect}
              tools={l.tools}
              webStatus={l.webStatus}
              lastCheck={l.lastCheck}
              busy={busy}
              onOpenGuide={handleOpenGuide}
              onDetectTools={handleDetectTools}
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
              onRepair={l.repair}
              onTerminal={l.openTerminal}
              onUninstall={async () => {
                await l.loadPreview();
                setUninstallOpen(true);
              }}
              onPlugins={() => setPluginsOpen(true)}
            />

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
        onOpenGuide={handleOpenGuide}
        onCancel={() => setInstallOpen(false)}
        onConfirm={async (dir) => {
          // 点击「开始安装」后立即关闭对话框：进度与报错在日志面板实时展示。
          setInstallOpen(false);
          void l.install(dir);
        }}
      />

      <UninstallDialog
        preview={l.preview}
        open={uninstallOpen}
        busy={l.phase === "uninstalling"}
        onCancel={() => setUninstallOpen(false)}
        onConfirm={async (sel) => {
          await l.uninstall(sel);
          setUninstallOpen(false);
        }}
      />

      {l.manualCandidates && l.manualCandidates.length > 0 && (
        <ManualInstallModal
          candidates={l.manualCandidates}
          busy={busy}
          onAdopt={l.adoptInstall}
          onIgnore={l.ignoreManualInstall}
        />
      )}

      <SettingsDrawer
        open={settingsOpen}
        config={l.config}
        dark={dark}
        onClose={() => setSettingsOpen(false)}
        onSave={l.saveSettings}
        onTheme={onTheme}
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
