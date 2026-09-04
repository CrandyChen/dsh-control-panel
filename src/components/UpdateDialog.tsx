// 更新详情对话框：以简洁列表列出所有已安装内核（安装方式 / 当前版本 / 新版本），
// 由用户勾选要更新的内核（可跨安装方式多选）；更新后勾选版本的旧内核将被替换。
// web 服务运行中时提示更新前会自动停止。

import { CloudSyncOutlined } from "@ant-design/icons";
import { Alert, Button, Flex, Modal, Space, Table, Tag, Typography } from "antd";
import type { ColumnsType, TableProps } from "antd/es/table";
import { useState } from "react";
import { useI18n } from "../i18n";
import type { UpdateCheckResult, UpdateKernelInfo } from "../types";

interface Props {
  result: UpdateCheckResult;
  /** web 服务是否在运行（运行中提示更新前会自动停止）。 */
  webRunning: boolean;
  /** 是否正在执行更新（对话框保持打开并显示进度提示）。 */
  updating: boolean;
  onIgnore: () => void;
  /** 更新：勾选的内核 id 列表。更新后旧版本内核会被替换/删除。 */
  onUpdate: (selectedIds: string[]) => void;
}

export default function UpdateDialog({
  result,
  webRunning,
  updating,
  onIgnore,
  onUpdate,
}: Props) {
  const { t } = useI18n();
  const rows = result.kernels ?? [];
  // 默认勾选有更新的内核（已是最新/查询失败的行不可勾选）。
  const defaultSelected = rows.filter((r) => r.updateAvailable).map((r) => r.id);
  const [selected, setSelected] = useState<string[]>(defaultSelected);

  const modeLabel = (mode: string) =>
    mode === "source" ? t("status.mode.source") : t("status.mode.prebuilt");

  const columns: ColumnsType<UpdateKernelInfo> = [
    {
      key: "mode",
      title: t("update.col.mode"),
      dataIndex: "mode",
      render: (_: unknown, r: UpdateKernelInfo) => modeLabel(r.mode),
    },
    {
      key: "current",
      title: t("update.col.current"),
      dataIndex: "currentVersion",
      render: (v: string) => <Typography.Text code>{v}</Typography.Text>,
    },
    {
      key: "latest",
      title: t("update.col.latest"),
      render: (_: unknown, r: UpdateKernelInfo) =>
        r.updateAvailable ? (
          <Typography.Text code>{r.latestVersion}</Typography.Text>
        ) : r.latestVersion ? (
          <Tag color="default">{t("update.upToDate")}</Tag>
        ) : (
          <Tag color="warning">{t("update.checkFailed")}</Tag>
        ),
    },
  ];

  const rowSelection: TableProps<UpdateKernelInfo>["rowSelection"] = {
    selectedRowKeys: selected,
    onChange: (keys) => setSelected(keys as string[]),
    getCheckboxProps: (record) => ({ disabled: !record.updateAvailable }),
  };

  const confirm = () => {
    // 仅提交有更新的勾选内核（禁用/已是最新的行不可能被勾选，双保险过滤）。
    const ids = rows
      .filter((r) => r.updateAvailable && selected.includes(r.id))
      .map((r) => r.id);
    onUpdate(ids);
  };

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
      width={620}
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
            onClick={confirm}
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
        <Typography.Text type="secondary" style={{ fontSize: 12 }}>
          {t("update.selectHint")}
        </Typography.Text>
        <Table<UpdateKernelInfo>
          rowKey="id"
          size="small"
          pagination={false}
          columns={columns}
          dataSource={rows}
          rowSelection={rowSelection}
          scroll={{ y: 260 }}
        />
        <Typography.Text type="secondary" style={{ fontSize: 12 }}>
          {t("update.replaceHint")}
        </Typography.Text>
      </Space>
    </Modal>
  );
}
