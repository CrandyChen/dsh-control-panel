// 卸载确认：勾选清单（默认全选）→ 二次确认 → 执行。
// 清单为粗粒度两项：安装目录 + DSH 用户数据目录（~/.dsh）。
// 清单统计（可能耗时较长）在后台进行：对话框先显示 loading 与实时进度，
// 统计完成后再展示勾选清单；统计期间可取消扫描。

import { DeleteOutlined } from "@ant-design/icons";
import {
  Alert,
  Button,
  Checkbox,
  Flex,
  Modal,
  Space,
  Spin,
  Tag,
  Typography,
} from "antd";
import { useState } from "react";
import { useI18n } from "../i18n";
import type { UninstallPreview } from "../types";
import { formatSize } from "../types";

interface Props {
  preview: UninstallPreview | null;
  open: boolean;
  busy: boolean;
  /** 是否正在后台统计卸载清单（生成预览）。 */
  loading: boolean;
  /** 统计过程中的最新进度行（后端推送）。 */
  progress: string | null;
  onCancel: () => void;
  /** 取消后台统计扫描（loading 状态下 footer 的「取消」按钮）。 */
  onCancelScan: () => void;
  onConfirm: (selected: string[]) => Promise<void>;
}

export default function UninstallDialog({
  preview,
  open,
  busy,
  loading,
  progress,
  onCancel,
  onCancelScan,
  onConfirm,
}: Props) {
  const { t } = useI18n();
  const [checked, setChecked] = useState<string[]>([]);

  const confirm = async () => {
    await onConfirm(checked);
    setChecked([]);
  };

  return (
    <Modal
      title={
        <Space>
          <DeleteOutlined style={{ color: "#ff4d4f" }} />
          {t("uninstall.title")}
        </Space>
      }
      open={open}
      onCancel={loading ? onCancelScan : onCancel}
      width={680}
      footer={
        loading ? (
          <Flex justify="end">
            <Button onClick={onCancelScan}>{t("common.cancel")}</Button>
          </Flex>
        ) : (
          <Flex justify="end" gap={8}>
            <Button onClick={onCancel} disabled={busy}>
              {t("uninstall.cancel")}
            </Button>
            <Button
              danger
              type="primary"
              icon={<DeleteOutlined />}
              disabled={checked.length === 0 || busy}
              loading={busy}
              onClick={confirm}
            >
              {busy ? t("uninstall.deleting") : t("uninstall.delete", { 0: checked.length })}
            </Button>
          </Flex>
        )
      }
      destroyOnClose
    >
      {loading ? (
        <Space direction="vertical" size={16} style={{ width: "100%" }}>
          <Flex align="center" gap={12}>
            <Spin />
            <Typography.Text>{t("uninstall.scanning")}</Typography.Text>
          </Flex>
          {progress && (
            <Typography.Text type="secondary" style={{ fontSize: 12 }}>
              {progress}
            </Typography.Text>
          )}
        </Space>
      ) : (
        <Space direction="vertical" size={12} style={{ width: "100%" }}>
          <Alert
            type="error"
            showIcon
            message={t("uninstall.alert.title")}
            description={
              <div style={{ fontSize: 13, whiteSpace: "pre-line", lineHeight: 1.8 }}>
                {t("uninstall.alert.desc")}
              </div>
            }
          />
          {preview && preview.entries.length === 0 && (
            <Typography.Text type="secondary" style={{ fontSize: 12 }}>
              {t("uninstall.empty")}
            </Typography.Text>
          )}
          <Checkbox.Group
            style={{ width: "100%" }}
            value={checked}
            onChange={(v) => setChecked(v as string[])}
          >
            <Space direction="vertical" style={{ width: "100%" }}>
              {preview?.entries.map((e) => (
                <Checkbox key={e.id} value={e.path} style={{ width: "100%" }}>
                  <Flex align="center" justify="space-between" gap={8} style={{ width: "100%" }}>
                    <Typography.Text style={{ fontSize: 13 }} ellipsis={{ tooltip: e.path }}>
                      {e.name}
                    </Typography.Text>
                    <Space size={6}>
                      <Tag color={e.kind === "directory" ? "blue" : "default"}>
                        {e.kind === "directory" ? t("uninstall.dir") : t("uninstall.file")}
                      </Tag>
                      <Tag>{formatSize(e.size)}</Tag>
                      {e.kind === "directory" && <Tag>{t("uninstall.items", { 0: e.items })}</Tag>}
                    </Space>
                  </Flex>
                </Checkbox>
              ))}
              {preview && preview.entries.length === 0 && (
                <Typography.Text type="secondary">{t("uninstall.noItems")}</Typography.Text>
              )}
            </Space>
          </Checkbox.Group>
          <Typography.Text type="secondary" style={{ fontSize: 12 }}>
            {t("uninstall.pnpmNote")}
          </Typography.Text>
        </Space>
      )}
    </Modal>
  );
}
