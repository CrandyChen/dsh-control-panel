// 安装向导弹窗：选择安装方式（默认预构建内核）+ 可选内核版本。
// - 预构建内核（默认）：列出 GitHub 发布的可安装版本，用户选择一个版本安装；
//   已安装的同 (方式,版本) 标「已安装」，其「安装」按钮变为「修复安装」。
// - 源码安装：仅安装最新版本（git clone 默认分支）；已安装时按钮变为「修复安装」。
// 点击「安装 / 修复安装」后对话框立即关闭，进度与报错在日志面板实时展示。

import { Alert, Button, Flex, Modal, Radio, Segmented, Space, Tag, Typography } from "antd";
import { useEffect, useMemo, useState } from "react";
import { api } from "../api";
import { MODE2_DIR_NAME, REPO_DIR_NAME } from "../constants";
import { useI18n } from "../i18n";
import type { InstallMode, KernelInstall, PrebuiltRelease, ToolStatus } from "../types";

interface Props {
  open: boolean;
  /** 运行环境工具检测结果（源码模式 git 缺失时为拦截项）。 */
  tools: ToolStatus[] | null;
  /** 当前安装方式（默认 prebuilt）。 */
  installMode: InstallMode;
  /** 已安装内核注册表（用于标记已安装版本 + 修复安装）。 */
  installedKernels: KernelInstall[];
  /** 打开 Git 安装指引 tab。 */
  onOpenGuide: () => void;
  onCancel: () => void;
  /** 安装：mode + 版本（prebuilt 指定版本；source 传 null）。 */
  onInstall: (mode: string, version: string | null) => Promise<void>;
  /** 修复安装：指定内核 id（null 表示当前活动内核）。 */
  onRepair: (kernelId: string | null) => Promise<void>;
}

export default function InstallModal({
  open,
  tools,
  installMode,
  installedKernels,
  onOpenGuide,
  onCancel,
  onInstall,
  onRepair,
}: Props) {
  const { t } = useI18n();
  const [mode, setMode] = useState<InstallMode>(installMode);
  // 程序运行目录（exe 所在目录，后端返回）：仅用于展示固定安装位置。
  const [parentDir, setParentDir] = useState("");
  // 预构建可选版本列表（来自 GitHub release）。
  const [releases, setReleases] = useState<PrebuiltRelease[]>([]);
  const [loadingReleases, setLoadingReleases] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [selectedVersion, setSelectedVersion] = useState<string | null>(null);

  useEffect(() => {
    if (open) {
      api
        .getDefaultParentDir()
        .then((d) => {
          if (d) setParentDir(d);
        })
        .catch(() => {
          /* 获取失败则不展示路径 */
        });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  useEffect(() => {
    if (!open) {
      setLoadError(null);
      return;
    }
    if (mode !== "prebuilt") return;
    setLoadingReleases(true);
    setLoadError(null);
    api
      .listPrebuiltReleases()
      .then((list) => {
        setReleases(list);
        setSelectedVersion(list.length > 0 ? list[0].version : null);
      })
      .catch((e) => setLoadError(String(e)))
      .finally(() => setLoadingReleases(false));
  }, [open, mode]);

  // 必装项（源码模式仅 Git）缺失或版本过低 → 拦截安装，引导查看指引。
  const sourceBlocked = useMemo(
    () =>
      mode === "source" &&
      (tools ?? []).some((tool) => tool.required && (!tool.installed || !tool.ok)),
    [tools, mode],
  );

  // 判断当前所选版本是否已安装（同 方式+版本）。
  const sourceInstalled = useMemo(
    () => installedKernels.some((k) => k.mode === "source"),
    [installedKernels],
  );
  const selectedInstalled = useMemo(() => {
    if (mode === "source") return sourceInstalled;
    if (!selectedVersion) return false;
    return installedKernels.some((k) => k.mode === "prebuilt" && k.version === selectedVersion);
  }, [mode, selectedVersion, installedKernels, sourceInstalled]);

  const selectedKernelId = useMemo(() => {
    if (mode === "source") {
      const k = installedKernels.find((k) => k.mode === "source");
      return k ? k.id : null;
    }
    const k = installedKernels.find(
      (k) => k.mode === "prebuilt" && k.version === selectedVersion,
    );
    return k ? k.id : null;
  }, [mode, selectedVersion, installedKernels]);

  const target = parentDir.trim()
    ? `${parentDir.trim().replace(/[\\/]+$/, "")}\\${REPO_DIR_NAME}`
    : null;

  const confirm = async () => {
    if (selectedInstalled) {
      await onRepair(selectedKernelId);
    } else if (mode === "prebuilt") {
      await onInstall("prebuilt", selectedVersion);
    } else {
      await onInstall("source", null);
    }
  };

  const actionLabel = selectedInstalled ? t("install.repair") : t("install.start");

  // 预构建已安装列表（供「已安装」标识与列表展示）。
  const installedPrebuiltVersions = useMemo(
    () => new Set(installedKernels.filter((k) => k.mode === "prebuilt").map((k) => k.version)),
    [installedKernels],
  );

  return (
    <Modal
      title={t("install.title")}
      open={open}
      onCancel={onCancel}
      width={680}
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
          <>
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
            {loadError ? (
              <Alert
                type="warning"
                showIcon
                message={t("install.versions.loadFail")}
                description={<div style={{ fontSize: 13 }}>{loadError}</div>}
              />
            ) : null}
            {loadingReleases ? (
              <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                {t("install.versions.loading")}
              </Typography.Text>
            ) : releases.length > 0 ? (
              <Radio.Group
                style={{ width: "100%" }}
                value={selectedVersion ?? undefined}
                onChange={(e) => setSelectedVersion(e.target.value as string)}
              >
                <Space direction="vertical" style={{ width: "100%" }}>
                  {releases.map((r) => {
                    const installed = installedPrebuiltVersions.has(r.version);
                    return (
                      <Radio key={r.tag} value={r.version} style={{ width: "100%" }}>
                        <Flex align="center" justify="space-between" gap={8} style={{ width: "100%" }}>
                          <Typography.Text style={{ fontSize: 13 }}>
                            {t("install.version.label", { 0: r.version })}
                          </Typography.Text>
                          <Space size={6}>
                            {r.prerelease && <Tag color="orange">{t("install.version.prerelease")}</Tag>}
                            {installed && <Tag color="green">{t("install.version.installed")}</Tag>}
                          </Space>
                        </Flex>
                      </Radio>
                    );
                  })}
                </Space>
              </Radio.Group>
            ) : (
              !loadError && (
                <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                  {t("install.versions.empty")}
                </Typography.Text>
              )
            )}
          </>
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
            {sourceInstalled && (
              <Tag color="green" style={{ marginInlineEnd: 0 }}>
                {t("install.version.installed")}
              </Tag>
            )}
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
              {actionLabel}
            </Button>
          )}
        </Flex>
      </Space>
    </Modal>
  );
}
