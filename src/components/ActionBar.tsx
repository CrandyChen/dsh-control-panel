// 操作按钮组：安装 / 启动 / 停止 / 更新（先检测后弹详情对话框）/ 修复安装 / 插件 /
// 终端 / 卸载（按状态联动启停）。更新按钮有新版时显示红色 NEW 徽标。

import {
  AppstoreOutlined,
  CheckCircleOutlined,
  CloudDownloadOutlined,
  CloudSyncOutlined,
  ConsoleSqlOutlined,
  DeleteOutlined,
  GlobalOutlined,
  PoweroffOutlined,
  ReloadOutlined,
} from "@ant-design/icons";
import { App, Badge, Button, Flex, Space, Tooltip, theme } from "antd";
import { useState } from "react";
import { useI18n } from "../i18n";
import type {
  AppConfig,
  DetectResult,
  Phase,
  PluginUpdates,
  UpdateCheckResult,
  WebStatus,
} from "../types";
import UpdateDialog from "./UpdateDialog";

interface Props {
  config: AppConfig | null;
  detect: DetectResult | null;
  phase: Phase;
  webStatus: WebStatus;
  onInstallClick: () => void;
  onStart: () => void;
  onOpenUi: () => void;
  onStop: () => void;
  onUpdate: () => void;
  /** 检测新版本：返回结果（失败为 null）；有新版时由本组件弹详情对话框。 */
  onCheck: () => Promise<UpdateCheckResult | null>;
  onTerminal: () => void;
  onUninstall: () => void;
  /** 打开插件管理对话框。 */
  onPlugins: () => void;
  /** 最近一次插件更新检测结果（有为「新版本」的插件时按钮显示 NEW 徽标）。 */
  pluginUpdates: PluginUpdates | null;
}

export default function ActionBar({
  config,
  detect,
  phase,
  webStatus,
  onInstallClick,
  onStart,
  onOpenUi,
  onStop,
  onUpdate,
  onCheck,
  onTerminal,
  onUninstall,
  onPlugins,
  pluginUpdates,
}: Props) {
  const { message, modal } = App.useApp();
  const { token } = theme.useToken();
  const { t } = useI18n();
  const [updateResult, setUpdateResult] = useState<UpdateCheckResult | null>(null);
  const installed = detect?.installed ?? false;
  const ready = webStatus === "ready";
  const starting = webStatus === "starting";
  const running = ready || starting;
  const busy = phase !== "idle";
  const updateAvailable = config?.updateAvailable ?? false;
  const installMode = config?.installMode ?? "prebuilt";
  const pluginUpdateAvailable = (pluginUpdates?.entries ?? []).some((e) => e.updateAvailable);

  /** 合并后的「更新」：先检测 → 有新版本弹详情对话框 → 用户选择执行或忽略。 */
  const handleUpdateClick = async () => {
    if (busy) return;
    const r = await onCheck();
    if (!r) return;
    if (!r.updateAvailable) {
      message.success(t("action.upToDate"));
      return;
    }
    setUpdateResult(r);
  };

  const confirmStop = () => {
    modal.confirm({
      title: t("action.stop.confirm.title"),
      content: t("action.stop.confirm.content"),
      okText: t("action.stop.confirm.ok"),
      okButtonProps: { danger: true },
      cancelText: t("action.stop.confirm.cancel"),
      onOk: onStop,
    });
  };

  return (
    <Flex wrap gap={12} align="center">
      <Tooltip title={t("action.install.tip")}>
        <Button
          type="primary"
          size="large"
          icon={<CloudDownloadOutlined />}
          disabled={busy}
          onClick={onInstallClick}
        >
          {t("action.install")}
        </Button>
      </Tooltip>

      {starting ? (
        <Tooltip title={t("action.starting.tip")}>
          <Button
            type="primary"
            size="large"
            icon={<PoweroffOutlined />}
            loading
            disabled
          >
            {t("action.starting")}
          </Button>
        </Tooltip>
      ) : ready ? (
        <Button
          size="large"
          icon={<GlobalOutlined />}
          disabled={busy}
          onClick={onOpenUi}
        >
          {t("action.openUi")}
        </Button>
      ) : (
        <Tooltip
          title={installed ? t("action.start.tip") : t("action.start.tip.notInstalled")}
        >
          <Button
            type="primary"
            size="large"
            icon={<PoweroffOutlined />}
            disabled={!installed || busy}
            onClick={onStart}
          >
            {t("action.start")}
          </Button>
        </Tooltip>
      )}

      <Tooltip title={t("action.stop.tip")}>
        <Button
          size="large"
          icon={<CheckCircleOutlined />}
          danger
          disabled={!running || busy}
          onClick={confirmStop}
        >
          {t("action.stop")}
        </Button>
      </Tooltip>

      <Badge count={updateAvailable ? "NEW" : 0} color="red" size="small" offset={[-6, 10]}>
        <Tooltip
          title={installed ? t("action.update.tip") : t("action.start.tip.notInstalled")}
        >
          <Button
            size="large"
            icon={<CloudSyncOutlined />}
            loading={phase === "checking"}
            disabled={!installed || running || busy}
            onClick={() => void handleUpdateClick()}
          >
            {t("action.update")}
          </Button>
        </Tooltip>
      </Badge>

      <Badge count={pluginUpdateAvailable ? "NEW" : 0} color="red" size="small" offset={[-6, 10]}>
        <Tooltip
          title={
            pluginUpdateAvailable
              ? t("action.plugins.updatesTip")
              : t("action.plugins.tip")
          }
        >
          <Button
            size="large"
            icon={<AppstoreOutlined />}
            disabled={!installed || busy}
            onClick={onPlugins}
          >
            {t("action.plugins")}
          </Button>
        </Tooltip>
      </Badge>

      <Tooltip title={t("action.terminal.tip")}>
        <Button
          size="large"
          icon={<ConsoleSqlOutlined />}
          disabled={!installed || busy}
          onClick={onTerminal}
        >
          {t("action.terminal")}
        </Button>
      </Tooltip>

      <Tooltip title={t("action.uninstall.tip")}>
        <Button
          size="large"
          danger
          icon={<DeleteOutlined />}
          disabled={!installed || running || busy}
          onClick={onUninstall}
        >
          {t("action.uninstall")}
        </Button>
      </Tooltip>

      {busy && (
        <Space>
          <ReloadOutlined spin style={{ color: token.colorPrimary }} />
          <span style={{ color: token.colorTextSecondary, fontSize: 13 }}>
            {t("action.busy")}
          </span>
        </Space>
      )}

      {updateResult && (
        <UpdateDialog
          result={updateResult}
          currentVersion={detect?.version ?? config?.installedVersion ?? null}
          webRunning={running}
          installMode={installMode}
          updating={phase === "updating"}
          onIgnore={() => setUpdateResult(null)}
          onUpdate={() => {
            setUpdateResult(null);
            onUpdate();
          }}
        />
      )}
    </Flex>
  );
}
