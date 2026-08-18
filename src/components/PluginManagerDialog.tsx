// 插件管理对话框：已安装插件列表（可多选卸载）/ 智能输入安装 / 单条更新 / 全部卸载 /
// 推荐插件链接。命令由后端执行（dsh plugin，自动处理 pnpm dsh 切换与构建拦截重试），
// 输出流实时展示在「执行详情」，操作完成后自动刷新列表。
// web 服务运行中：仅允许安装新插件，更新 / 卸载被禁用。

import {
  AppstoreOutlined,
  LinkOutlined,
  ReloadOutlined,
} from "@ant-design/icons";
import type { ColumnsType } from "antd/es/table";
import {
  Alert,
  App,
  Button,
  Collapse,
  Flex,
  Input,
  Modal,
  Space,
  Spin,
  Table,
  Tag,
  Tooltip,
  Typography,
} from "antd";
import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "../api";
import { AWESOME_PLUGINS_URL } from "../constants";
import { useI18n } from "../i18n";
import type { AppConfig, PipelineEvent, PluginEntry, PluginList, PluginOpResult } from "../types";

interface Props {
  open: boolean;
  config: AppConfig | null;
  /** web 服务是否在运行（运行中禁止更新/卸载，仅可安装）。 */
  webRunning: boolean;
  onClose: () => void;
  /** 持久化设置（如修改默认 profile）。 */
  onSaveSettings: (patch: Partial<AppConfig>) => Promise<void>;
}

/** 执行详情保留的最大行数。 */
const MAX_DETAIL = 300;

interface DetailLine {
  stream: string;
  line: string;
}

export default function PluginManagerDialog({ open, config, webRunning, onClose, onSaveSettings }: Props) {
  const { message, modal } = App.useApp();
  const { t } = useI18n();
  const [profile, setProfile] = useState(config?.pluginProfile ?? "web");
  const [list, setList] = useState<PluginList | null>(null);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [busyLabel, setBusyLabel] = useState<string | null>(null);
  const [input, setInput] = useState("");
  const [result, setResult] = useState<{ ok: boolean; message: string } | null>(null);
  const [detail, setDetail] = useState<DetailLine[]>([]);
  const [detailOpen, setDetailOpen] = useState(false);
  const [selectedKeys, setSelectedKeys] = useState<React.Key[]>([]);

  /** 读取指定 profile 的插件列表。 */
  const loadList = useCallback(
    async (p: string) => {
      setLoading(true);
      try {
        const res = await api.pluginList(p);
        setList(res);
        setSelectedKeys([]);
      } catch (e) {
        setList(null);
        message.error(t("plugin.list.fail", { 0: String(e) }));
      } finally {
        setLoading(false);
      }
    },
    [message, t],
  );

  // 打开时：同步配置中的 profile 并加载列表，重置操作状态。
  // 仅在打开瞬间读取 config；之后 profile 变更由 saveProfile 自行保存并刷新。
  useEffect(() => {
    if (open) {
      setProfile(config?.pluginProfile || "web");
      setResult(null);
      setDetail([]);
      setDetailOpen(false);
      setInput("");
      void loadList(config?.pluginProfile || "web");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  /** 后端管道事件：收集输出行到执行详情。 */
  const onEvent = useCallback((e: PipelineEvent) => {
    if (e.type === "output") {
      setDetail((prev) => {
        const next = [...prev, { stream: e.stream, line: e.line }];
        return next.length > MAX_DETAIL ? next.slice(next.length - MAX_DETAIL) : next;
      });
    }
  }, []);

  /** 通用操作包装：置忙 → 执行 → 结果提示 → 成功后清空输入并刷新列表。返回执行结果（失败为 null）。 */
  const runOp = useCallback(
    async (label: string, run: () => Promise<PluginOpResult>): Promise<PluginOpResult | null> => {
      setBusy(true);
      setBusyLabel(label);
      setResult(null);
      setDetail([]);
      setDetailOpen(false);
      try {
        const r = await run();
        setResult({ ok: r.ok, message: r.message });
        if (r.ok) {
          setInput("");
          await loadList(profile);
        }
        return r;
      } catch (e) {
        setResult({ ok: false, message: String(e) });
        setDetailOpen(true);
        return null;
      } finally {
        setBusy(false);
        setBusyLabel(null);
      }
    },
    [loadList, profile],
  );

  /** 智能安装：输入由后端解析（npm 包名 / github 标识 / GitHub 链接 / 完整命令）。 */
  const install = useCallback(() => {
    const value = input.trim();
    if (!value) {
      message.warning(t("plugin.install.warn"));
      return;
    }
    void runOp(t("plugin.op.install"), () => api.pluginInstall(value, profile, onEvent)).then((r) => {
      // 运行中安装成功后提示重启 DSH 使插件生效。
      if (r?.ok && webRunning) {
        message.info(t("plugin.install.restartHint"), 5);
      }
    });
  }, [input, profile, onEvent, runOp, message, webRunning, t]);

  const updateOne = useCallback(
    (entry: PluginEntry) => {
      modal.confirm({
        title: t("plugin.update.confirm.title", { 0: entry.key }),
        content: (
          <div style={{ fontSize: 13 }}>
            {t("plugin.update.confirm.exec", { 0: profile, 1: entry.key })}
            <br />
            {t("plugin.update.confirm.note")}
          </div>
        ),
        okText: t("plugin.update"),
        cancelText: t("common.cancel"),
        onOk: () =>
          runOp(
            t("plugin.update.confirm.title", { 0: entry.key }),
            () => api.pluginUpdate(entry.key, profile, onEvent),
          ),
      });
    },
    [modal, profile, runOp, onEvent, t],
  );

  const removeOne = useCallback(
    (entry: PluginEntry) => {
      modal.confirm({
        title: t("plugin.remove.confirm.title", { 0: entry.key }),
        content: (
          <div style={{ fontSize: 13 }}>
            {t("plugin.remove.confirm.desc", { 0: profile, 1: entry.key })}
          </div>
        ),
        okText: t("plugin.remove"),
        okButtonProps: { danger: true },
        cancelText: t("common.cancel"),
        onOk: () =>
          runOp(
            t("plugin.remove.confirm.title", { 0: entry.key }),
            () => api.pluginRemove([entry.key], profile, onEvent),
          ),
      });
    },
    [modal, profile, runOp, onEvent, t],
  );

  const removeSelected = useCallback(() => {
    if (selectedKeys.length === 0) return;
    const keys = selectedKeys.map(String);
    modal.confirm({
      title: t("plugin.remove.selected.title", { 0: keys.length }),
      content: (
        <div style={{ fontSize: 13 }}>
          {t("plugin.remove.confirm.desc", { 0: profile, 1: keys.join(" ") })}
        </div>
      ),
      okText: t("plugin.remove"),
      okButtonProps: { danger: true },
      cancelText: t("common.cancel"),
      onOk: () =>
        runOp(t("plugin.op.remove"), () => api.pluginRemove(keys, profile, onEvent)),
    });
  }, [selectedKeys, modal, profile, runOp, onEvent, t]);

  const removeAll = useCallback(() => {
    const keys = (list?.entries ?? []).map((e) => e.key);
    if (keys.length === 0) {
      message.info(t("plugin.remove.none"));
      return;
    }
    modal.confirm({
      title: t("plugin.remove.all.title"),
      content: (
        <div style={{ fontSize: 13 }}>
          {t("plugin.remove.all.desc1", { 0: keys.length })}
          <br />
          {t("plugin.remove.confirm.desc", { 0: profile, 1: keys.join(" ") })}
          <br />
          {t("plugin.remove.all.desc2")}
        </div>
      ),
      okText: t("plugin.remove.all.btn"),
      okButtonProps: { danger: true },
      cancelText: t("common.cancel"),
      onOk: () => runOp(t("plugin.removeAll"), () => api.pluginRemove(keys, profile, onEvent)),
    });
  }, [list, modal, profile, runOp, onEvent, message, t]);

  /** 修改 profile：立即保存到配置并刷新列表（不同 profile 互相隔离）。 */
  const saveProfile = useCallback(
    (p: string) => {
      const v = p.trim() || "web";
      setProfile(v);
      void onSaveSettings({ pluginProfile: v });
      void loadList(v);
    },
    [onSaveSettings, loadList],
  );

  /** 推荐插件列表：GitHub 禁止内嵌 iframe，在系统浏览器打开。 */
  const openRecommendation = useCallback(async () => {
    try {
      await api.openExternal(AWESOME_PLUGINS_URL);
    } catch (e) {
      message.error(t("plugin.recommend.fail", { 0: String(e) }));
    }
  }, [message, t]);

  const rows = useMemo(() => (list?.entries ?? []).map((e) => ({ ...e })), [list]);

  const columns: ColumnsType<PluginEntry> = [
    {
      title: t("plugin.col.name"),
      dataIndex: "key",
      render: (v: string) => (
        <Typography.Text copyable={{ text: v }} style={{ fontFamily: "monospace" }}>
          {v}
        </Typography.Text>
      ),
    },
    {
      title: t("plugin.col.type"),
      dataIndex: "isBundle",
      width: 110,
      render: (b: boolean) =>
        b ? <Tag color="blue">{t("plugin.col.bundle")}</Tag> : <Tag>{t("plugin.col.dep")}</Tag>,
    },
    {
      title: t("plugin.col.spec"),
      dataIndex: "spec",
      render: (v: string) => (
        <Typography.Text copyable={{ text: v }} style={{ fontFamily: "monospace", fontSize: 12 }}>
          {v || "—"}
        </Typography.Text>
      ),
    },
    {
      title: t("plugin.col.action"),
      width: 140,
      render: (_, record) => (
        <Space size={4}>
          <Button
            size="small"
            disabled={busy || webRunning}
            onClick={() => updateOne(record)}
          >
            {t("plugin.update")}
          </Button>
          <Button
            size="small"
            danger
            type="text"
            disabled={busy || webRunning}
            onClick={() => removeOne(record)}
          >
            {t("plugin.remove")}
          </Button>
        </Space>
      ),
    },
  ];

  return (
    <Modal
      title={t("plugin.title")}
      open={open}
      onCancel={onClose}
      width={800}
      footer={<Button onClick={onClose}>{t("plugin.close")}</Button>}
      destroyOnClose
    >
      <Space direction="vertical" size={12} style={{ width: "100%" }}>
        {/* profile 与命令执行方式 */}
        <Flex align="center" gap={8} wrap>
          <Typography.Text type="secondary" style={{ fontSize: 12, whiteSpace: "nowrap" }}>
            profile
          </Typography.Text>
          <Input
            size="small"
            style={{ width: 140 }}
            value={profile}
            disabled={busy}
            onChange={(e) => setProfile(e.target.value)}
            onBlur={(e) => saveProfile(e.target.value)}
            onPressEnter={(e) => saveProfile(e.currentTarget.value)}
          />
          <Tag color={list?.usePnpmDsh ? "orange" : "green"} style={{ marginInlineEnd: 0 }}>
            {list?.usePnpmDsh ? t("plugin.exec.pnpm") : t("plugin.exec.dsh")}
          </Tag>
          <Tooltip title={t("plugin.profile.tip")}>
            <Typography.Text type="secondary" style={{ fontSize: 12 }}>
              {t("plugin.profile.isolated")}
            </Typography.Text>
          </Tooltip>
        </Flex>

        {webRunning && (
          <Alert
            type="warning"
            showIcon
            message={t("plugin.running.msg")}
            description={t("plugin.running.desc")}
          />
        )}
        {busy && (
          <Alert
            type="info"
            showIcon
            message={t("plugin.busy.msg", { 0: busyLabel ?? "…" })}
            description={t("plugin.busy.desc")}
          />
        )}
        {result && (
          <Alert
            type={result.ok ? "success" : "error"}
            showIcon
            message={result.message}
            closable
            onClose={() => setResult(null)}
          />
        )}

        {/* 安装输入 + 推荐链接 */}
        <Flex justify="space-between" align="center" gap={8} wrap>
          <Typography.Text type="secondary" style={{ fontSize: 13 }}>
            {t("plugin.input.hint")}
          </Typography.Text>
          <Tooltip title={t("plugin.recommend.tip")}>
            <Button
              size="small"
              type="link"
              icon={<LinkOutlined />}
              onClick={() => void openRecommendation()}
            >
              {t("plugin.recommend")}
            </Button>
          </Tooltip>
        </Flex>
        <Flex gap={8}>
          <Input
            placeholder={t("plugin.input.placeholder")}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onPressEnter={() => install()}
            disabled={busy}
            allowClear
          />
          <Button
            type="primary"
            icon={<AppstoreOutlined />}
            disabled={busy}
            onClick={install}
          >
            {t("plugin.install.btn")}
          </Button>
        </Flex>

        {/* 已安装插件列表 */}
        {loading ? (
          <Flex justify="center" style={{ padding: 24 }}>
            <Spin />
          </Flex>
        ) : list && !list.initialized ? (
          <Alert
            type="info"
            showIcon
            message={t("plugin.notInitialized.msg", { 0: profile })}
            description={t("plugin.notInitialized.desc", { 0: list.profileDir })}
          />
        ) : (
          <Table<PluginEntry>
            size="small"
            rowKey="key"
            columns={columns}
            dataSource={rows}
            pagination={false}
            rowSelection={{
              selectedRowKeys: selectedKeys,
              onChange: setSelectedKeys,
              getCheckboxProps: () => ({ disabled: busy || webRunning }),
            }}
            locale={{ emptyText: t("plugin.empty") }}
          />
        )}
        {list && list.builtinBundles.length > 0 && (
          <Flex align="center" gap={6} wrap>
            <Typography.Text type="secondary" style={{ fontSize: 12 }}>
              {t("plugin.builtin")}
            </Typography.Text>
            {list.builtinBundles.map((b) => (
              <Tag key={b} style={{ marginInlineEnd: 0 }}>
                {b}
              </Tag>
            ))}
          </Flex>
        )}

        {/* 卸载操作 */}
        <Flex justify="space-between" gap={8} wrap>
          <Space>
            <Button
              danger
              disabled={busy || webRunning || selectedKeys.length === 0}
              onClick={removeSelected}
            >
              {selectedKeys.length > 0
                ? t("plugin.removeSelected", { 0: selectedKeys.length })
                : t("plugin.removeSelectedNone")}
            </Button>
            <Button danger type="text" disabled={busy || webRunning} onClick={removeAll}>
              {t("plugin.removeAll")}
            </Button>
          </Space>
          <Button icon={<ReloadOutlined />} disabled={busy} onClick={() => void loadList(profile)}>
            {t("plugin.refresh")}
          </Button>
        </Flex>

        {/* 执行详情 */}
        <Collapse
          ghost
          items={[
            {
              key: "detail",
              label: t("plugin.detail", { 0: detail.length }),
              children: (
                <pre
                  style={{
                    maxHeight: 220,
                    overflow: "auto",
                    fontSize: 12,
                    lineHeight: 1.6,
                    margin: 0,
                    whiteSpace: "pre-wrap",
                    wordBreak: "break-all",
                  }}
                >
                  {detail.length === 0
                    ? t("plugin.detail.empty")
                    : detail.map((d) => d.line).join("\n")}
                </pre>
              ),
            },
          ]}
          activeKey={detailOpen ? ["detail"] : []}
          onChange={(k) => setDetailOpen(Array.isArray(k) && k.includes("detail"))}
        />
      </Space>
    </Modal>
  );
}
