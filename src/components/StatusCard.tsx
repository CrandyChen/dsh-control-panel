// 状态卡片：安装状态 / 版本 / 更新 / 时间 / 运行状态 / 目录 / 运行环境工具。

import {
  BookOutlined,
  CloudSyncOutlined,
  FolderOpenOutlined,
  ReloadOutlined,
  RocketOutlined,
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
import { useState } from "react";
import { useI18n } from "../i18n";
import type {
  AppConfig,
  DetectResult,
  ToolStatus,
  UpdateCheckResult,
  WebStatus,
} from "../types";

interface Props {
  config: AppConfig | null;
  detect: DetectResult | null;
  /** 运行环境工具检测结果（git/node/pnpm 必装 + python 推荐）。 */
  tools: ToolStatus[] | null;
  webStatus: WebStatus;
  lastCheck: UpdateCheckResult | null;
  busy: boolean;
  /** 打开安装指引 tab。 */
  onOpenGuide: () => void;
  /** 手动重新检测运行环境（git/node/pnpm/python）。 */
  onDetectTools: () => Promise<void>;
}

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

/** 单个工具的状态标签。 */
function ToolTag({ tool }: { tool: ToolStatus }) {
  const { t } = useI18n();
  if (tool.installed && !tool.ok) {
    return (
      <Tooltip title={tool.detail ?? t("status.tool.low")}>
        <Tag color="orange" style={{ marginInlineEnd: 0 }}>
          {tool.name} ⚠ {tool.version}
        </Tag>
      </Tooltip>
    );
  }
  if (tool.installed) {
    return (
      <Tag color="green" style={{ marginInlineEnd: 0 }}>
        {tool.name} ✓ {tool.version}
      </Tag>
    );
  }
  // 未安装：必装项红色，推荐项（python）蓝色。
  return (
    <Tag color={tool.required ? "red" : "blue"} style={{ marginInlineEnd: 0 }}>
      {tool.name} {tool.required ? t("status.tool.missing") : t("status.tool.optional")}
    </Tag>
  );
}

export default function StatusCard({
  config,
  detect,
  tools,
  webStatus,
  lastCheck,
  busy,
  onOpenGuide,
  onDetectTools,
}: Props) {
  const { token } = theme.useToken();
  const { t } = useI18n();
  const installed = detect?.installed ?? false;
  const updateAvailable = config?.updateAvailable ?? false;
  const updateSubject = config?.latestSubject || lastCheck?.subject || null;
  const anyToolMissing = (tools ?? []).some((tool) => !tool.installed || !tool.ok);
  const [detecting, setDetecting] = useState(false);

  /** 手动触发运行环境检测：按钮转圈，完成后刷新标签。 */
  const handleDetect = async () => {
    if (detecting) return;
    setDetecting(true);
    try {
      await onDetectTools();
    } finally {
      setDetecting(false);
    }
  };

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
      styles={{ body: { padding: "12px 16px" } }}
    >
      <Descriptions
        size="small"
        column={2}
        colon={false}
        labelStyle={{ width: 110, color: token.colorTextSecondary }}
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
            key: "version",
            label: t("status.version"),
            children: (
              <Flex align="center" gap={6}>
                {/* 使用实时探测值（config 中的版本可能因手动 git pull 而陈旧） */}
                <Typography.Text copyable={!!detect?.version}>
                  {detect?.version ?? "—"}
                </Typography.Text>
                {detect?.installedCommit && (
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
        ]}
      />

      {/* 运行环境：git / node / pnpm（必装）+ python（推荐），缺失时可查看安装指引 */}
      <div
        style={{
          marginTop: 10,
          paddingTop: 10,
          borderTop: `1px solid ${token.colorBorderSecondary}`,
        }}
      >
        <Flex align="center" justify="space-between" gap={8} wrap>
          <Flex align="center" gap={8} wrap>
            <Typography.Text type="secondary" style={{ fontSize: 12, whiteSpace: "nowrap" }}>
              {t("status.env")}
            </Typography.Text>
            {tools === null && (
              <Tag color="default" style={{ marginInlineEnd: 0 }}>
                {t("status.env.checking")}
              </Tag>
            )}
            {tools?.map((tool) => (
              <ToolTag key={tool.id} tool={tool} />
            ))}
          </Flex>
          <Flex align="center" gap={4} wrap>
            <Button
              size="small"
              icon={<ReloadOutlined />}
              loading={detecting}
              onClick={() => void handleDetect()}
            >
              {t("status.env.detect")}
            </Button>
            {anyToolMissing && (
              <Button size="small" type="link" icon={<BookOutlined />} onClick={onOpenGuide}>
                {t("status.env.guide")}
              </Button>
            )}
          </Flex>
        </Flex>
      </div>
    </Card>
  );
}
