// 安装向导弹窗：选择安装方式（默认预构建内核）+ 对应参数。
// - 预构建内核（默认）：从 GitHub 下载并解压到程序运行目录下的 dsh 子目录，无需 Git / pnpm。
// - 源码安装：选择父目录（默认程序运行目录），自动创建 deepseek-harness 子目录；需 Git。
// 点击「开始安装」后对话框立即关闭，进度与报错在日志面板实时展示。

import { FolderOpenOutlined } from "@ant-design/icons";
import { Alert, App, Button, Flex, Input, Modal, Segmented, Space, Typography } from "antd";
import { useEffect, useMemo, useState } from "react";
import { api } from "../api";
import { MODE2_DIR_NAME, REPO_DIR_NAME } from "../constants";
import { useI18n } from "../i18n";
import type { InstallMode, ToolStatus } from "../types";

interface Props {
  open: boolean;
  /** 运行环境工具检测结果（源码模式 git 缺失时为拦截项）。 */
  tools: ToolStatus[] | null;
  /** 当前安装方式（默认 prebuilt）。 */
  installMode: InstallMode;
  /** 打开 Git 安装指引 tab。 */
  onOpenGuide: () => void;
  onCancel: () => void;
  onConfirm: (dir: string, mode: string) => Promise<void>;
}

export default function InstallModal({
  open,
  tools,
  installMode,
  onOpenGuide,
  onCancel,
  onConfirm,
}: Props) {
  const { message } = App.useApp();
  const { t } = useI18n();
  const [dir, setDir] = useState("");
  const [mode, setMode] = useState<InstallMode>(installMode);

  // 打开弹窗时：若源码模式的父目录为空，预填「程序运行目录」（后端返回，含 exe 所在目录）。
  useEffect(() => {
    if (open && !dir.trim()) {
      api
        .getDefaultParentDir()
        .then((d) => {
          if (d) setDir(d);
        })
        .catch(() => {
          /* 获取失败则不预填 */
        });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const pick = async () => {
    const d = await api.pickDirectory();
    if (d) setDir(d);
  };

  // 必装项（源码模式仅 Git）缺失或版本过低 → 拦截安装，引导查看指引。
  const sourceBlocked = useMemo(
    () =>
      mode === "source" &&
      (tools ?? []).some((tool) => tool.required && (!tool.installed || !tool.ok)),
    [tools, mode],
  );

  const target = dir.trim() ? `${dir.trim().replace(/[\\/]+$/, "")}\\${REPO_DIR_NAME}` : null;

  const confirm = async () => {
    if (mode === "source") {
      if (!dir.trim()) {
        message.warning(t("install.pick.first"));
        return;
      }
      await onConfirm(dir.trim(), "source");
    } else {
      await onConfirm("", "prebuilt");
    }
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
        <Segmented
          block
          value={mode}
          onChange={(v) => setMode(v as InstallMode)}
          options={[
            { label: t("install.mode.prebuilt"), value: "prebuilt" },
            { label: t("install.mode.source"), value: "source" },
          ]}
        />

        {mode === "prebuilt" ? (
          <Alert
            type="info"
            showIcon
            message={t("install.prebuilt.title")}
            description={
              <div style={{ fontSize: 13 }}>
                <code>{t("install.prebuilt.desc")}</code>
                <br />
                {t("install.prebuilt.autoDir", { 0: MODE2_DIR_NAME })}
              </div>
            }
          />
        ) : (
          <>
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
                  <br />
                  {t("install.info.requiresGit")}
                </div>
              }
            />
            {sourceBlocked && (
              <Alert
                type="warning"
                showIcon
                message={t("install.blocked.title")}
                description={
                  <div style={{ fontSize: 13 }}>
                    {(tools ?? [])
                      .filter((tool) => tool.required && (!tool.installed || !tool.ok))
                      .map((tool) => (
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
                disabled={sourceBlocked}
              />
              <Button icon={<FolderOpenOutlined />} onClick={pick} disabled={sourceBlocked}>
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
          </>
        )}

        <Flex justify="end" gap={8}>
          <Button onClick={onCancel}>{t("install.cancel")}</Button>
          {sourceBlocked ? (
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
