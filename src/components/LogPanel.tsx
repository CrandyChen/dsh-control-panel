// 日志面板：实时滚动、级别着色、可折叠、可清空，展示日志文件路径。
// 颜色全部取自 antd theme token，深色 / 浅色模式下均可读。

import { ClearOutlined, ConsoleSqlOutlined } from "@ant-design/icons";
import { Button, Collapse, Flex, Space, theme, Typography } from "antd";
import { useEffect, useRef } from "react";
import { useI18n } from "../i18n";
import type { LogLine } from "../types";

interface Props {
  logs: LogLine[];
  busy: boolean;
  currentStep: string | null;
  logDir: string;
  onClear: () => void;
}

const LEVEL_COLOR: Record<string, string> = {
  ERROR: "#ff4d4f",
  WARN: "#faad14",
};

export default function LogPanel({ logs, busy, currentStep, logDir, onClear }: Props) {
  const { token } = theme.useToken();
  const { t } = useI18n();
  const boxRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = boxRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [logs]);

  return (
    <div
      style={{
        border: `1px solid ${token.colorBorderSecondary}`,
        borderRadius: 8,
        background: token.colorFillQuaternary,
      }}
    >
      <Flex align="center" justify="space-between" style={{ padding: "6px 12px" }}>
        <Flex align="center" gap={8}>
          <ConsoleSqlOutlined style={{ color: token.colorTextSecondary }} />
          <Typography.Text style={{ fontSize: 13 }}>
            {t("log.title", { 0: logs.length })}
          </Typography.Text>
          {busy && currentStep && (
            <Typography.Text type="warning" style={{ fontSize: 12 }}>
              {currentStep}
            </Typography.Text>
          )}
        </Flex>
        <Button size="small" type="text" icon={<ClearOutlined />} onClick={onClear}>
          {t("log.clear")}
        </Button>
      </Flex>
      <Collapse
        ghost
        size="small"
        defaultActiveKey={["log"]}
        items={[
          {
            key: "log",
            label: (
              <span style={{ fontSize: 12, color: token.colorTextTertiary }}>
                {t("log.toggle")}
              </span>
            ),
            children: (
              <div
                ref={boxRef}
                style={{
                  height: 200,
                  overflowY: "auto",
                  background: token.colorFillTertiary,
                  borderRadius: 8,
                  padding: "8px 12px",
                  fontFamily: "Consolas, 'Courier New', monospace",
                  fontSize: 12,
                  lineHeight: 1.6,
                  color: token.colorText,
                }}
              >
                {logs.length === 0 && (
                  <Typography.Text type="secondary">{t("log.empty")}</Typography.Text>
                )}
                {logs.map((l, i) => (
                  <div key={i} style={{ whiteSpace: "pre-wrap", wordBreak: "break-all" }}>
                    <Space size={6} style={{ width: "100%" }}>
                      {l.ts && (
                        <Typography.Text type="secondary" style={{ fontSize: 11 }}>
                          {l.ts}
                        </Typography.Text>
                      )}
                      <span style={{ color: LEVEL_COLOR[l.level] ?? "inherit" }}>{l.text}</span>
                    </Space>
                  </div>
                ))}
              </div>
            ),
          },
        ]}
      />
      <div style={{ padding: "2px 12px 8px" }}>
        <Typography.Text
          type="secondary"
          style={{ fontSize: 11 }}
          ellipsis={{ tooltip: logDir }}
        >
          {t("log.fileNote")}
          {logDir || "—"}
        </Typography.Text>
      </div>
    </div>
  );
}
