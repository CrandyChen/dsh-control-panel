// 安装向导弹窗：选择父目录 → 确认安装（将自动创建 deepseek-harness 子目录）。
// 点击「开始安装」后对话框立即关闭，进度与报错在日志面板实时展示。

import { FolderOpenOutlined } from "@ant-design/icons";
import { Alert, App, Button, Flex, Input, Modal, Space, Typography } from "antd";
import { useMemo, useState } from "react";
import { api } from "../api";
import { REPO_DIR_NAME } from "../constants";
import { useI18n } from "../i18n";
import type { ToolStatus } from "../types";

interface Props {
  open: boolean;
  /** 运行环境工具检测结果（用于展示必装项缺失警告，兜底拦截）。 */
  tools: ToolStatus[] | null;
  /** 打开安装指引 tab。 */
  onOpenGuide: () => void;
  onCancel: () => void;
  onConfirm: (dir: string) => Promise<void>;
}

export default function InstallModal({ open, tools, onOpenGuide, onCancel, onConfirm }: Props) {
  const { message } = App.useApp();
  const { t } = useI18n();
  const [dir, setDir] = useState("");

  const pick = async () => {
    const d = await api.pickDirectory();
    if (d) setDir(d);
  };

  // 必装项（git/node/pnpm）缺失或版本过低 → 拦截安装，引导查看指引。
  const requiredMissing = useMemo(
    () => (tools ?? []).filter((tool) => tool.required && (!tool.installed || !tool.ok)),
    [tools],
  );
  const blocked = requiredMissing.length > 0;

  const target = dir.trim() ? `${dir.trim().replace(/[\\/]+$/, "")}\\${REPO_DIR_NAME}` : null;

  const confirm = async () => {
    if (!dir.trim()) {
      message.warning(t("install.pick.first"));
      return;
    }
    await onConfirm(dir.trim());
  };

  return (
    <Modal
      title={t("install.title")}
      open={open}
      onCancel={onCancel}
      width={640}
      footer={null}
      destroyOnClose
    >
      <Space direction="vertical" size={14} style={{ width: "100%" }}>
        <Alert
          type="info"
          showIcon
          message={t("install.info.title")}
          description={
            <div style={{ fontSize: 13 }}>
              <code>{t("install.info.clone")}</code>
              <br />
              <code>{t("install.info.deps")}</code>
              <br />
              {t("install.info.autoDir", { 0: REPO_DIR_NAME })}
            </div>
          }
        />

        {blocked && (
          <Alert
            type="warning"
            showIcon
            message={t("install.blocked.title")}
            description={
              <div style={{ fontSize: 13 }}>
                {requiredMissing.map((tool) => (
                  <div key={tool.id}>
                    {tool.name}：
                    {tool.installed
                      ? t("install.blocked.low", { 0: tool.version ?? "" })
                      : t("install.blocked.missing")}
                    {tool.detail ? ` — ${tool.detail}` : ""}
                  </div>
                ))}
                <div style={{ marginTop: 4 }}>{t("install.blocked.desc")}</div>
              </div>
            }
          />
        )}

        <Flex gap={8}>
          <Input
            placeholder={t("install.dir.placeholder")}
            value={dir}
            onChange={(e) => setDir(e.target.value)}
            onPressEnter={() => void confirm()}
            disabled={blocked}
          />
          <Button icon={<FolderOpenOutlined />} onClick={pick} disabled={blocked}>
            {t("install.browse")}
          </Button>
        </Flex>
        {target && (
          <Typography.Text type="secondary" style={{ fontSize: 12 }}>
            {t("install.target", { 0: target })}
          </Typography.Text>
        )}
        <Typography.Text type="secondary" style={{ fontSize: 12 }}>
          {t("install.hint", { 0: REPO_DIR_NAME })}
        </Typography.Text>
        <Flex justify="end" gap={8}>
          <Button onClick={onCancel}>{t("install.cancel")}</Button>
          {blocked ? (
            <Button type="primary" onClick={onOpenGuide}>
              {t("install.guide")}
            </Button>
          ) : (
            <Button type="primary" onClick={() => void confirm()}>
              {t("install.start")}
            </Button>
          )}
        </Flex>
      </Space>
    </Modal>
  );
}
