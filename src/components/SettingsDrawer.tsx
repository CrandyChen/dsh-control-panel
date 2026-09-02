// 设置抽屉：自动检测 / 间隔 / 主题 / 语言 / 打开方式 / 软件更新 / 关于。

import { App, Button, Drawer, Flex, Form, InputNumber, Progress, Radio, Segmented, Switch, Typography } from "antd";
import { useState } from "react";
import { APP_VERSION } from "../constants";
import { useI18n } from "../i18n";
import type { LangSetting } from "../i18n";
import type { AppConfig, AppUpdateState } from "../types";

interface Props {
  open: boolean;
  config: AppConfig | null;
  /** 控制面板自身更新状态（后台检测/下载/就绪/失败）。 */
  appUpdate: AppUpdateState | null;
  onClose: () => void;
  onSave: (patch: Partial<AppConfig>) => Promise<void>;
  /** 手动检测一次控制面板更新。 */
  onCheckAppUpdate: () => Promise<void>;
}

export default function SettingsDrawer({ open, config, appUpdate, onClose, onSave, onCheckAppUpdate }: Props) {
  const { message } = App.useApp();
  const { t } = useI18n();
  const [autoCheck, setAutoCheck] = useState(config?.autoCheckEnabled ?? true);
  const [interval, setInterval] = useState(config?.autoCheckIntervalHours ?? 12);
  const [pluginAutoCheck, setPluginAutoCheck] = useState(config?.pluginAutoCheckEnabled ?? true);
  const [pluginInterval, setPluginInterval] = useState(
    config?.pluginAutoCheckIntervalHours ?? 12,
  );

  const save = async (patch: Partial<AppConfig>) => {
    await onSave(patch);
  };

  const updatingBusy = appUpdate?.status === "checking" || appUpdate?.status === "downloading";
  const updatePercent =
    appUpdate?.status === "downloading" && (appUpdate.total ?? 0) > 0
      ? Math.min(100, Math.round(((appUpdate.downloaded ?? 0) / (appUpdate.total ?? 1)) * 100))
      : undefined;

  const appUpdateStatusText = (() => {
    switch (appUpdate?.status) {
      case "checking":
        return t("settings.appUpdate.checking");
      case "downloading":
        return updatePercent != null
          ? t("settings.appUpdate.downloading", { 0: updatePercent })
          : t("settings.appUpdate.downloadingUnknown");
      case "ready":
        return t("settings.appUpdate.ready");
      case "upToDate":
        return t("settings.appUpdate.upToDate");
      case "failed":
        return t("settings.appUpdate.failed");
      default:
        return t("settings.appUpdate.idle");
    }
  })();

  return (
    <Drawer title={t("settings.title")} open={open} onClose={onClose} width={420} destroyOnClose={false}>
      <Form layout="vertical" size="middle">
        <Form.Item label={t("settings.autoCheck")} extra={t("settings.autoCheck.extra")}>
          <Flex align="center" justify="space-between">
            <Switch
              checked={autoCheck}
              onChange={(v) => {
                setAutoCheck(v);
                void save({ autoCheckEnabled: v });
              }}
            />
          </Flex>
        </Form.Item>
        <Form.Item label={t("settings.interval")}>
          <InputNumber
            min={1}
            max={168}
            value={interval}
            disabled={!autoCheck}
            onChange={(v) => {
              if (v == null) return;
              setInterval(v);
              void save({ autoCheckIntervalHours: v });
            }}
            addonAfter={t("settings.interval.unit")}
          />
        </Form.Item>
        <Form.Item
          label={t("settings.pluginAutoCheck")}
          extra={t("settings.pluginAutoCheck.extra")}
        >
          <Flex align="center" justify="space-between">
            <Switch
              checked={pluginAutoCheck}
              onChange={(v) => {
                setPluginAutoCheck(v);
                void save({ pluginAutoCheckEnabled: v });
              }}
            />
          </Flex>
        </Form.Item>
        <Form.Item label={t("settings.pluginInterval")}>
          <InputNumber
            min={1}
            max={168}
            value={pluginInterval}
            disabled={!pluginAutoCheck}
            onChange={(v) => {
              if (v == null) return;
              setPluginInterval(v);
              void save({ pluginAutoCheckIntervalHours: v });
            }}
            addonAfter={t("settings.pluginInterval.unit")}
          />
        </Form.Item>
        <Form.Item label={t("settings.appUpdate.title")} extra={t("settings.appUpdate.extra")}>
          <Flex vertical gap={8}>
            <Flex align="center" justify="space-between" gap={8}>
              <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                {t("settings.appUpdate.current", { 0: APP_VERSION })}
              </Typography.Text>
              <Button
                size="small"
                loading={updatingBusy}
                disabled={updatingBusy}
                onClick={() => void onCheckAppUpdate()}
              >
                {t("settings.appUpdate.check")}
              </Button>
            </Flex>
            {appUpdate?.status === "downloading" && (
              <Progress size="small" percent={updatePercent} />
            )}
            <Typography.Text type="secondary" style={{ fontSize: 12 }}>
              {appUpdateStatusText}
            </Typography.Text>
          </Flex>
        </Form.Item>
        <Form.Item label={t("settings.theme")}>
          <Segmented
            value={config?.theme ?? "auto"}
            options={[
              { label: t("settings.theme.auto"), value: "auto" },
              { label: t("settings.theme.light"), value: "light" },
              { label: t("settings.theme.dark"), value: "dark" },
            ]}
            onChange={(v) => void save({ theme: v as AppConfig["theme"] })}
          />
        </Form.Item>
        <Form.Item
          label={t("settings.language")}
          extra={config?.language === "auto" ? `${t("settings.language.auto")}: ${t("app.tag")}` : undefined}
        >
          <Segmented
            value={config?.language ?? "auto"}
            options={[
              { label: t("settings.language.auto"), value: "auto" },
              { label: t("settings.language.zh"), value: "zh-CN" },
              { label: t("settings.language.en"), value: "en" },
            ]}
            onChange={(v) => void save({ language: v as LangSetting })}
          />
        </Form.Item>
        <Form.Item label={t("settings.openUiMode")} extra={t("settings.openUiMode.extra")}>
          <Radio.Group
            value={config?.openUiMode ?? "tab"}
            onChange={(e) => void save({ openUiMode: e.target.value })}
            options={[
              { label: t("settings.openUiMode.tab"), value: "tab" },
              { label: t("settings.openUiMode.browser"), value: "browser" },
            ]}
            optionType="button"
            buttonStyle="solid"
          />
        </Form.Item>
        <Form.Item label={t("settings.installDir")}>
          <Typography.Text type="secondary">
            {config?.installDir ?? t("settings.installDir.none")}
          </Typography.Text>
        </Form.Item>
      </Form>

      <Flex vertical gap={4} style={{ marginTop: 24 }}>
        <Typography.Text type="secondary" style={{ fontSize: 12 }}>
          {t("settings.about", { 0: APP_VERSION })}
        </Typography.Text>
        <Typography.Text type="secondary" style={{ fontSize: 12 }}>
          {t("settings.about2")}
        </Typography.Text>
        <Typography.Text
          type="secondary"
          style={{ fontSize: 12 }}
          onClick={() => message.info("https://github.com/deepseek-ai/deepseek-harness")}
        >
          {t("settings.repo")}
        </Typography.Text>
      </Flex>
    </Drawer>
  );
}
