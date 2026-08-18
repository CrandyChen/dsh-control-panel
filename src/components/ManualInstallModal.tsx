// 手动安装检测弹窗：发现本机手动安装的 DeepSeek Harness 时询问是否采用。

import { ToolOutlined } from "@ant-design/icons";
import { Alert, App, Button, Flex, Modal, Radio, Space, Typography } from "antd";
import { useState } from "react";
import { useI18n } from "../i18n";

interface Props {
  candidates: string[];
  busy: boolean;
  onAdopt: (path: string) => Promise<void>;
  onIgnore: () => void;
}

export default function ManualInstallModal({ candidates, busy, onAdopt, onIgnore }: Props) {
  const { message } = App.useApp();
  const { t } = useI18n();
  const [selected, setSelected] = useState<string>(candidates[0] ?? "");

  const adopt = async () => {
    if (!selected) {
      message.warning(t("manual.select"));
      return;
    }
    await onAdopt(selected);
  };

  return (
    <Modal
      title={
        <Space>
          <ToolOutlined style={{ color: "#1677ff" }} />
          {t("manual.title")}
        </Space>
      }
      open
      closable={false}
      maskClosable={false}
      width={640}
      footer={
        <Flex justify="end" gap={8}>
          <Button onClick={onIgnore} disabled={busy}>
            {t("manual.ignore")}
          </Button>
          <Button type="primary" loading={busy} onClick={adopt}>
            {t("manual.adopt")}
          </Button>
        </Flex>
      }
    >
      <Space direction="vertical" size={14} style={{ width: "100%" }}>
        <Alert
          type="info"
          showIcon
          message={t("manual.alert.title")}
          description={<div style={{ fontSize: 13 }}>{t("manual.alert.desc")}</div>}
        />
        <Radio.Group
          value={selected}
          onChange={(e) => setSelected(e.target.value)}
          style={{ width: "100%" }}
        >
          <Space direction="vertical" style={{ width: "100%" }}>
            {candidates.map((p) => (
              <Radio key={p} value={p}>
                <Typography.Text style={{ fontSize: 13 }} copyable>
                  {p}
                </Typography.Text>
              </Radio>
            ))}
          </Space>
        </Radio.Group>
        <Typography.Text type="secondary" style={{ fontSize: 12 }}>
          {t("manual.hint")}
        </Typography.Text>
      </Space>
    </Modal>
  );
}
