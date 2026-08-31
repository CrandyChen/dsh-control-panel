// 状态卡片：安装状态 / 版本 / 更新 / 时间 / 运行状态 / 安装方式 / 目录。
// 右上角提供紧凑的「设置」入口。

import {
  CloudSyncOutlined,
  FolderOpenOutlined,
  RocketOutlined,
  SettingOutlined,
} from "@ant-design/icons";
import {
  Badge,
  Button,
  Card,
  Descriptions,
  Flex,
  Tag,
  theme,
  Tooltip,
  Typography,
} from "antd";
import { useI18n } from "../i18n";
import { api } from "../api";
import { formatMoney } from "../types";
import type {
  AppConfig,
  BalanceResult,
  DetectResult,
  InstallMode,
  UpdateCheckResult,
  WebStatus,
} from "../types";

interface Props {
  config: AppConfig | null;
  detect: DetectResult | null;
  webStatus: WebStatus;
  lastCheck: UpdateCheckResult | null;
  busy: boolean;
  /** 当前余额（安装 DSH 且配置 API 后每 5 分钟查询）。 */
  balance: BalanceResult | null;
  /** 打开设置抽屉。 */
  onOpenSettings: () => void;
}

/** 充值地址（与后端 balance::TOP_UP_URL 一致）。 */
const TOP_UP_URL = "https://platform.deepseek.com/top_up";

const STATUS_KEYS: Record<WebStatus, string> = {
  idle: "status.running.idle",
  starting: "status.running.starting",
  ready: "status.running.ready",
  stopped: "status.running.stopped",
  error: "status.running.error",
};

const STATUS_COLORS: Record<WebStatus, string> = {
  idle: "default",
  starting: "processing",
  ready: "success",
  stopped: "default",
  error: "error",
};

export default function StatusCard({
  config,
  detect,
  webStatus,
  lastCheck,
  busy,
  balance,
  onOpenSettings,
}: Props) {
  const { token } = theme.useToken();
  const { t } = useI18n();
  const installed = detect?.installed ?? false;
  const updateAvailable = config?.updateAvailable ?? false;
  const updateSubject = config?.latestSubject || lastCheck?.subject || null;
  const mode: InstallMode = config?.installMode ?? "prebuilt";

  return (
    <Card
      size="small"
      title={
        <Flex align="center" gap={8}>
          <RocketOutlined style={{ color: token.colorPrimary }} />
          <span>{t("status.title")}</span>
          {updateAvailable && <Badge status="error" />}
        </Flex>
      }
      extra={
        <Tooltip title={t("settings.title")}>
          <Button
            type="text"
            size="small"
            icon={<SettingOutlined />}
            aria-label={t("settings.title")}
            onClick={onOpenSettings}
          />
        </Tooltip>
      }
      styles={{ body: { padding: "12px 16px" } }}
    >
      <Descriptions
        size="small"
        column={{ xs: 1, sm: 2, lg: 3, xl: 3 }}
        colon={false}
        labelStyle={{ width: 90, color: token.colorTextSecondary }}
        items={[
          {
            key: "installed",
            label: t("status.installed"),
            children: (
              <Tag color={installed ? "green" : "red"} style={{ marginInlineEnd: 0 }}>
                {installed ? t("status.installed.yes") : t("status.installed.no")}
              </Tag>
            ),
          },
          {
            key: "running",
            label: t("status.running"),
            children: (
              <Flex align="center" gap={6}>
                <Tag color={STATUS_COLORS[webStatus]} style={{ marginInlineEnd: 0 }}>
                  {t(STATUS_KEYS[webStatus])}
                </Tag>
                {busy && <Tag color="blue">{t("status.busy")}</Tag>}
              </Flex>
            ),
          },
          {
            key: "mode",
            label: t("status.mode"),
            children: (
              <Tag color="blue" style={{ marginInlineEnd: 0 }}>
                {mode === "prebuilt" ? t("status.mode.prebuilt") : t("status.mode.source")}
              </Tag>
            ),
          },
          {
            key: "kernels",
            label: t("status.kernels"),
            children: (
              <Tag color="cyan" style={{ marginInlineEnd: 0 }}>
                {t("status.kernels.count", { 0: config?.installedKernels.length ?? 0 })}
              </Tag>
            ),
          },
          {
            key: "version",
            label: t("status.version"),
            children: (
              <Flex align="center" gap={6}>
                {/* 使用实时探测值（config 中的版本可能因手动 git pull 而陈旧） */}
                <Typography.Text copyable={!!detect?.version}>
                  {detect?.version ?? "—"}
                </Typography.Text>
                {mode === "source" && detect?.installedCommit && (
                  <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                    {detect.installedCommit.slice(0, 7)}
                  </Typography.Text>
                )}
              </Flex>
            ),
          },
          {
            key: "update",
            label: t("status.update"),
            children: !config?.lastCheckAt ? (
              <Tag color="default" style={{ marginInlineEnd: 0 }}>
                {t("status.update.none")}
              </Tag>
            ) : updateAvailable ? (
              <Tooltip title={updateSubject ?? t("status.update.tip")}>
                <Tag color="red" style={{ marginInlineEnd: 0 }}>
                  {t("status.update.available")}
                </Tag>
              </Tooltip>
            ) : (
              <Tag color="default" style={{ marginInlineEnd: 0 }}>
                {t("status.update.latest")}
              </Tag>
            ),
          },
          {
            key: "updatedAt",
            label: t("status.updatedAt"),
            children: config?.lastUpdatedAt ?? "—",
          },
          {
            key: "checkedAt",
            label: t("status.checkedAt"),
            children: (
              <Flex align="center" gap={6}>
                <span>{config?.lastCheckAt ?? "—"}</span>
                {lastCheck && lastCheck.behind > 0 && (
                  <Tag color="orange" style={{ marginInlineEnd: 0 }}>
                    {t("status.behind", { 0: lastCheck.behind })}
                  </Tag>
                )}
              </Flex>
            ),
          },
          {
            key: "installDir",
            label: t("status.installDir"),
            children: (
              <Flex align="center" gap={6} style={{ width: "100%", minWidth: 0, alignItems: "flex-start" }}>
                <FolderOpenOutlined style={{ color: token.colorTextTertiary, marginTop: 3 }} />
                <Typography.Text
                  style={{
                    fontSize: 13,
                    display: "block",
                    maxWidth: "100%",
                    wordBreak: "break-all",
                    whiteSpace: "normal",
                  }}
                  copyable={!!detect?.installDir}
                >
                  {detect?.installDir ?? t("status.installDir.none")}
                </Typography.Text>
              </Flex>
            ),
          },
          {
            key: "dshHome",
            label: t("status.dshHome"),
            children: (
              <Flex align="center" gap={6} style={{ width: "100%", minWidth: 0, alignItems: "flex-start" }}>
                <CloudSyncOutlined style={{ color: token.colorTextTertiary, marginTop: 3 }} />
                <Typography.Text
                  style={{
                    fontSize: 13,
                    display: "block",
                    maxWidth: "100%",
                    wordBreak: "break-all",
                    whiteSpace: "normal",
                  }}
                  copyable={!!detect?.dshHome}
                >
                  {detect?.dshHome ?? "—"}
                </Typography.Text>
              </Flex>
            ),
          },
          {
            key: "balance",
            label: t("status.balance"),
            children: (
              <Flex align="center" gap={8} wrap>
                {balance?.balance != null ? (
                  <Typography.Text
                    strong
                    style={{
                      color: balance.balance < 10 ? token.colorError : token.colorText,
                    }}
                  >
                    {formatMoney(balance.balance, balance.currency ?? "CNY")}
                  </Typography.Text>
                ) : balance?.apiKeySet === false ? (
                  <Typography.Text type="secondary">{t("status.balance.none")}</Typography.Text>
                ) : (
                  <Typography.Text type="secondary">—</Typography.Text>
                )}
                <Button
                  type="link"
                  size="small"
                  style={{ padding: 0, height: "auto", fontSize: 12 }}
                  onClick={() => void api.openExternal(TOP_UP_URL)}
                >
                  {t("status.balance.topUp")}
                </Button>
              </Flex>
            ),
          },
        ]}
      />
    </Card>
  );
}
