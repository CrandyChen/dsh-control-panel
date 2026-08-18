// 更新详情对话框：展示检测结果（当前/最新 commit、落后提交数、更新内容），
// 由用户选择「立即更新」或「忽略」。web 服务运行中时提示更新前会自动停止。

import { CloudSyncOutlined } from "@ant-design/icons";
import { Alert, Button, Descriptions, Flex, Modal, Space, Tag, Typography } from "antd";
import { useI18n } from "../i18n";
import type { UpdateCheckResult } from "../types";

interface Props {
  result: UpdateCheckResult;
  /** 当前安装版本（展示用；config.installedVersion）。 */
  currentVersion: string | null;
  /** web 服务是否在运行（运行中提示更新前会自动停止）。 */
  webRunning: boolean;
  /** 是否正在执行更新（对话框保持打开并显示进度提示）。 */
  updating: boolean;
  onIgnore: () => void;
  onUpdate: () => void;
}

export default function UpdateDialog({
  result,
  currentVersion,
  webRunning,
  updating,
  onIgnore,
  onUpdate,
}: Props) {
  const { t } = useI18n();
  return (
    <Modal
      title={
        <Space>
          <CloudSyncOutlined style={{ color: "#1677ff" }} />
          {t("update.title")}
        </Space>
      }
      open
      onCancel={onIgnore}
      width={560}
      maskClosable={!updating}
      footer={
        <Flex justify="end" gap={8}>
          <Button onClick={onIgnore} disabled={updating}>
            {t("update.ignore")}
          </Button>
          <Button
            type="primary"
            icon={<CloudSyncOutlined />}
            loading={updating}
            onClick={onUpdate}
          >
            {updating ? t("update.updating") : t("update.now")}
          </Button>
        </Flex>
      }
    >
      <Space direction="vertical" size={12} style={{ width: "100%" }}>
        {webRunning && (
          <Alert
            type="warning"
            showIcon
            message={t("update.running.msg")}
            description={t("update.running.desc")}
          />
        )}
        <Descriptions
          size="small"
          column={1}
          colon={false}
          labelStyle={{ width: 110 }}
          items={[
            {
              key: "v",
              label: t("update.currentVersion"),
              children: currentVersion ?? "—",
            },
            {
              key: "local",
              label: t("update.currentCommit"),
              children: (
                <Typography.Text code style={{ fontSize: 12 }}>
                  {result.localCommit.slice(0, 7)}
                </Typography.Text>
              ),
            },
            {
              key: "remote",
              label: t("update.latestCommit"),
              children: (
                <Typography.Text code style={{ fontSize: 12 }}>
                  {result.remoteCommit.slice(0, 7)}
                </Typography.Text>
              ),
            },
            {
              key: "behind",
              label: t("update.behind"),
              children: <Tag color="orange">{t("update.count", { 0: result.behind })}</Tag>,
            },
            {
              key: "subject",
              label: t("update.subject"),
              children: result.subject || "—",
            },
            {
              key: "at",
              label: t("update.checkedAt"),
              children: result.checkedAt,
            },
          ]}
        />
        <Typography.Text type="secondary" style={{ fontSize: 12 }}>
          {t("update.steps")}
        </Typography.Text>
      </Space>
    </Modal>
  );
}
