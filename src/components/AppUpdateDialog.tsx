// 软件升级对话框：当已有完整更新包（本次启动即升级）时自动打开。
// 展示「正在升级到 XXX 版」+ 阶段文字 + 进度条（解压 → 生成更新脚本 → 正在重启），
// 准备完成后调用 restartToUpdate（由 upater.cmd 在旧进程退出后替换并自动启动新版）。
// 可被取消/出错时允许关闭；网络异常等由后台静默处理，不弹本对话框。

import { Alert, Button, Flex, Modal, Progress, Tag, Typography } from "antd";
import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import { useI18n } from "../i18n";
import type { PipelineEvent } from "../types";

interface Props {
  open: boolean;
  targetVersion: string | null;
  onClose: () => void;
}

type Status = "running" | "restarting" | "error";

export default function AppUpdateDialog({ open, targetVersion, onClose }: Props) {
  const { t } = useI18n();
  const [label, setLabel] = useState("");
  const [percent, setPercent] = useState(0);
  const [status, setStatus] = useState<Status>("running");
  const [error, setError] = useState<string | null>(null);
  const startedRef = useRef(false);

  useEffect(() => {
    if (!open) {
      startedRef.current = false;
      setStatus("running");
      setError(null);
      setPercent(0);
      setLabel("");
      return;
    }
    if (startedRef.current) return;
    startedRef.current = true;
    setLabel(t("appUpdate.dialog.preparing"));
    setPercent(0);

    const onEvent = (e: PipelineEvent) => {
      if (e.type === "phase") {
        setLabel(e.title);
        setPercent(e.percent);
      } else if (e.type === "downloadProgress" && e.step === "app-extract") {
        // 解压进度归入对话框进度条（0→60%）。
        setPercent(e.total > 0 ? Math.min(60, Math.round((e.received / e.total) * 60)) : 0);
      } else if (e.type === "error") {
        setStatus("error");
        setError(e.message);
      }
    };

    api
      .applyAppUpdate(onEvent)
      .then(() => {
        setStatus("restarting");
        setPercent(100);
        setLabel(t("appUpdate.dialog.restarting"));
        // 给「正在重启…」一点展示时间，再触发更新脚本并退出。
        window.setTimeout(() => {
          void api.restartToUpdate().catch(() => {
            setStatus("error");
            setError(t("appUpdate.dialog.restartFail"));
          });
        }, 600);
      })
      .catch((e) => {
        setStatus("error");
        setError(String(e));
      });
  }, [open, targetVersion, t]);

  const closable = status === "error";

  return (
    <Modal
      title={t("appUpdate.dialog.title", { 0: targetVersion ?? "" })}
      open={open}
      onCancel={onClose}
      closable={closable}
      maskClosable={closable}
      width={520}
      centered
      footer={
        status === "error" ? (
          <Button onClick={onClose}>{t("common.cancel")}</Button>
        ) : null
      }
    >
      <Flex vertical gap={12}>
        <Flex align="center" gap={8} wrap>
          <Typography.Text strong style={{ fontSize: 14 }}>
            {label}
          </Typography.Text>
          {status === "restarting" ? (
            <Tag color="blue">{t("appUpdate.dialog.restartTag")}</Tag>
          ) : (
            <Tag color="processing">{t("appUpdate.dialog.runningTag")}</Tag>
          )}
        </Flex>
        <Progress
          size="small"
          status={status === "error" ? "exception" : "active"}
          percent={percent}
        />
        {status === "error" && error && <Alert type="error" showIcon message={error} />}
      </Flex>
    </Modal>
  );
}
