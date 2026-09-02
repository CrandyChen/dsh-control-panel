// 插件管理对话框：已安装插件列表（可多选批量操作）/ 智能输入安装 / 单条与批量更新 /
// 卸载 / 推荐插件链接。命令由后端直接在后台执行（无确认弹窗），执行过程以「阶段进度条」
// 展示（后端推送 Phase 事件驱动；原始子进程输出只落盘到 logs/，不涌入前端），
// 失败时进度条终止并在其下方给出精简错误。完成后自动刷新列表。
// 更新检测内置网络重试：单项查询失败时按退坡间隔自动复查，不展示失败标记、不误报「无更新」。

import {
  AppstoreOutlined,
  CloudSyncOutlined,
  DownOutlined,
  LinkOutlined,
  LoadingOutlined,
  ReloadOutlined,
  RightOutlined,
} from "@ant-design/icons";
import type { ColumnsType } from "antd/es/table";
import {
  Alert,
  App,
  Button,
  Flex,
  Input,
  Modal,
  Progress,
  Select,
  Space,
  Spin,
  Table,
  Tag,
  theme,
  Tooltip,
  Typography,
} from "antd";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import { AWESOME_PLUGINS_URL } from "../constants";
import { useI18n } from "../i18n";
import type {
  AppConfig,
  PipelineEvent,
  PluginEntry,
  PluginList,
  PluginOpResult,
  PluginUpdateInfo,
  PluginUpdates,
} from "../types";

interface Props {
  open: boolean;
  config: AppConfig | null;
  /** web 服务是否在运行（运行中禁止更新/卸载，仅可安装）。 */
  webRunning: boolean;
  onClose: () => void;
  /** 持久化设置（如修改默认 profile）。 */
  onSaveSettings: (patch: Partial<AppConfig>) => Promise<void>;
}

/** 更新检测失败后的自动复查间隔（毫秒）：8 秒 → 30 秒 → 2 分钟 → 10 分钟（封顶循环）。 */
const UPDATE_RETRY_DELAYS_MS = [8_000, 30_000, 120_000, 600_000];

/** 插件操作的分阶段进度状态（由后端 Phase/error 事件驱动，最终状态由 runOp 依据结果设定）。 */
interface OpProgress {
  /** 当前阶段文字（进度条上方展示）。 */
  label: string;
  /** 里程碑百分比（0-100）。 */
  percent: number;
  status: "running" | "success" | "error";
  /** 最终结果 / 精简失败信息（进度条下方展示）。 */
  message: string | null;
}

export default function PluginManagerDialog({ open, config, webRunning, onClose, onSaveSettings }: Props) {
  const { message } = App.useApp();
  const { t } = useI18n();
  const { token } = theme.useToken();
  const [profile, setProfile] = useState(config?.pluginProfile ?? "web");
  /** 本机已存在的 profile 列表（$DSH_HOME/profiles 下的目录），供下拉框选择。 */
  const [profiles, setProfiles] = useState<string[]>([]);
  const [list, setList] = useState<PluginList | null>(null);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  /** 后端实时推送的分阶段进度（非空时展示进度条）。 */
  const [op, setOp] = useState<OpProgress | null>(null);
  const [input, setInput] = useState("");
  const [guideOpen, setGuideOpen] = useState(false);
  const [selectedKeys, setSelectedKeys] = useState<React.Key[]>([]);
  /** 插件更新检测结果（按 key 索引）。 */
  const [updates, setUpdates] = useState<PluginUpdates | null>(null);
  const [checkingUpdates, setCheckingUpdates] = useState(false);

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

  /** 复查定时器与连续失败轮数（手动触发 / 切换 profile / 关闭弹窗时重置）。 */
  const retryTimer = useRef<number | null>(null);
  const retryAttempt = useRef(0);
  const checkUpdatesRef = useRef<((p: string) => Promise<void>) | null>(null);

  const clearRetry = useCallback(() => {
    if (retryTimer.current !== null) {
      window.clearTimeout(retryTimer.current);
      retryTimer.current = null;
    }
  }, []);

  /** 有失败项时安排一次退坡复查：间隔随连续失败轮数递增，成功后在 checkUpdates 内清零。 */
  const scheduleRetry = useCallback(
    (p: string) => {
      clearRetry();
      const idx = Math.min(retryAttempt.current, UPDATE_RETRY_DELAYS_MS.length - 1);
      const ms = UPDATE_RETRY_DELAYS_MS[idx];
      retryAttempt.current += 1;
      retryTimer.current = window.setTimeout(() => {
        retryTimer.current = null;
        void checkUpdatesRef.current?.(p);
      }, ms);
    },
    [clearRetry],
  );

  /** 检测当前 profile 的插件更新（网络查询；存在失败项时自动退坡复查，
   * 失败项不展示「检测失败」标记，也不误报「无更新」）。 */
  const checkUpdates = useCallback(
    async (p: string) => {
      clearRetry();
      setCheckingUpdates(true);
      try {
        const u = await api.pluginCheckUpdates(p);
        setUpdates(u);
        if ((u.entries ?? []).some((e) => e.error)) {
          scheduleRetry(p);
        } else {
          retryAttempt.current = 0;
        }
      } catch (e) {
        message.error(t("plugin.updates.fail", { 0: String(e) }), 5);
        scheduleRetry(p);
      } finally {
        setCheckingUpdates(false);
      }
    },
    [clearRetry, scheduleRetry, message, t],
  );
  checkUpdatesRef.current = checkUpdates;

  // 卸载组件 / 关闭弹窗时清理待执行的复查定时器。
  useEffect(() => clearRetry, [clearRetry]);

  // 打开时：同步配置中的 profile 并加载列表，重置操作状态。
  // 仅在打开瞬间读取 config；之后 profile 变更由 saveProfile 自行保存并刷新。
  useEffect(() => {
    if (open) {
      setProfile(config?.pluginProfile || "web");
      setOp(null);
      setInput("");
      setUpdates(null);
      clearRetry();
      retryAttempt.current = 0;
      void loadList(config?.pluginProfile || "web");
      void checkUpdates(config?.pluginProfile || "web");
      // 拉取本机已有 profile 供下拉框选择（失败时回退为仅当前 profile）。
      void api
        .pluginProfiles()
        .then(setProfiles)
        .catch(() => setProfiles([]));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  /** 后端管道事件：分阶段进度更新进度条；error 记录精简失败。
   * 原始子进程输出（output/stepStarted 等）不再展示，直接忽略；
   * finished 是逐条中间事件，不据此收尾（最终状态由 runOp 依据返回结果设定，
   * 避免批量任务里进度条被某条成功提前抬到 100 而回落）。 */
  const onEvent = useCallback((e: PipelineEvent) => {
    if (e.type === "phase") {
      setOp((prev) => (prev ? { ...prev, label: e.title, percent: e.percent } : prev));
    } else if (e.type === "error") {
      setOp((prev) => (prev ? { ...prev, status: "error", message: e.message } : prev));
    }
  }, []);

  /** 通用操作包装：置忙 → （若 web 运行则先自动停止）→ 以分阶段进度条展示执行过程 →
   * 结果提示（成功/失败显示在进度条下方）→ 成功后清空输入并刷新列表，
   * 结束后（无论成败）若之前运行则自动重新启动 web。返回执行结果（失败为 null）。 */
  const runOp = useCallback(
    async (label: string, run: () => Promise<PluginOpResult>): Promise<PluginOpResult | null> => {
      setBusy(true);
      setOp({ label, percent: 0, status: "running", message: null });
      const wasRunning = webRunning;
      try {
        if (wasRunning) {
          await api.stopWeb();
        }
        const r = await run();
        setOp((prev) => ({
          ...(prev ?? { label, percent: 0, status: "running", message: null }),
          status: r.ok ? "success" : "error",
          message: r.message,
          percent: r.ok ? 100 : prev?.percent ?? 0,
          label: r.ok ? t("plugin.progress.done") : t("plugin.progress.error"),
        }));
        if (r.ok) {
          setInput("");
          await loadList(profile);
        }
        return r;
      } catch (e) {
        setOp((prev) => ({
          label: t("plugin.progress.error"),
          percent: prev?.percent ?? 0,
          status: "error",
          message: String(e),
        }));
        return null;
      } finally {
        // 操作前停掉了 web，操作后自动恢复；不设置 startedByUs，避免自动打开 DSH Tab。
        if (wasRunning) {
          api.startWeb(null, () => undefined).catch(() => undefined);
        }
        setBusy(false);
      }
    },
    [loadList, profile, webRunning, t],
  );

  /** 智能安装：输入由后端解析（npm 包名 / github 标识 / GitHub 链接 / 完整命令）。
   * 初始 label 为「正在安装插件：<输入>」，随后由后端 Phase 事件驱动细化。 */
  const install = useCallback(() => {
    const value = input.trim();
    if (!value) {
      message.warning(t("plugin.install.warn"));
      return;
    }
    void runOp(t("plugin.install.named", { 0: value }), () =>
      api.pluginInstall(value, profile, onEvent),
    );
  }, [input, profile, onEvent, runOp, message, t]);

  /** 更新单个插件：直接后台执行，过程见进度条。 */
  const updateOne = useCallback(
    (entry: PluginEntry) => {
      void runOp(t("plugin.op.update"), () => api.pluginUpdate([entry.key], profile, onEvent));
    },
    [profile, runOp, onEvent, t],
  );

  /** 批量更新所选插件：逐个在后台执行，全程仅一次服务启停封装。 */
  const updateSelected = useCallback(() => {
    const keys = selectedKeys.map(String);
    if (keys.length === 0) return;
    void runOp(
      t("plugin.updateSelected.n", { 0: keys.length }),
      () => api.pluginUpdate(keys, profile, onEvent),
    );
  }, [selectedKeys, profile, runOp, onEvent, t]);

  const removeOne = useCallback(
    (entry: PluginEntry) => {
      void runOp(t("plugin.op.remove"), () => api.pluginRemove([entry.key], profile, onEvent));
    },
    [profile, runOp, onEvent, t],
  );

  const removeSelected = useCallback(() => {
    const keys = selectedKeys.map(String);
    if (keys.length === 0) return;
    void runOp(t("plugin.op.remove"), () => api.pluginRemove(keys, profile, onEvent));
  }, [selectedKeys, profile, runOp, onEvent, t]);

  /** 修改 profile：立即保存到配置并刷新列表（不同 profile 互相隔离）。 */
  const saveProfile = useCallback(
    (p: string) => {
      const v = p.trim() || "web";
      setProfile(v);
      setUpdates(null);
      clearRetry();
      retryAttempt.current = 0;
      void onSaveSettings({ pluginProfile: v });
      void loadList(v);
      void checkUpdates(v);
    },
    [onSaveSettings, loadList, checkUpdates, clearRetry],
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

  /** 下拉框选项：已存在的 profile + 当前选中值（保证选中项始终可见），去重排序。 */
  const profileOptions = useMemo(() => {
    const set = new Set<string>([...profiles, profile]);
    return [...set]
      .sort((a, b) => a.localeCompare(b))
      .map((v) => ({ value: v, label: v }));
  }, [profiles, profile]);

  /** 插件更新结果按 key 索引，供列表行展示「有新版本」。 */
  const updateMap = useMemo(() => {
    const m = new Map<string, PluginUpdateInfo>();
    (updates?.entries ?? []).forEach((e) => m.set(e.key, e));
    return m;
  }, [updates]);

  /** 可更新插件数量。 */
  const updatableCount = useMemo(
    () => (updates?.entries ?? []).filter((e) => e.updateAvailable).length,
    [updates],
  );

  /** 检测未完成（查询失败 / 已装版本不可读）的插件数量：既不当作「已是最新」也不当作「可更新」。 */
  const unconfirmedCount = useMemo(
    () => (updates?.entries ?? []).filter((e) => e.error).length,
    [updates],
  );

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
      render: (_, record) => {
        // 优先显示已安装的实际版本（GitHub 安装的插件由此显示真实版本号）；
        // 版本不可得时回退 spec；tooltip 展示版本与 spec 的完整信息。
        const display = record.version ?? record.spec;
        const tip =
          record.version && record.spec && record.spec !== record.version
            ? `${record.version} · ${record.spec}`
            : display;
        const info = updateMap.get(record.key);
        return (
          <Flex align="center" gap={6} wrap>
            <Typography.Text
              copyable={{ text: display }}
              style={{ fontFamily: "monospace", fontSize: 12 }}
              title={tip}
            >
              {display || "—"}
            </Typography.Text>
            {info?.updateAvailable && (
              <Tag color="orange" style={{ marginInlineEnd: 0 }}>
                {t("plugin.update.new", { 0: info.latestVersion ?? "" })}
              </Tag>
            )}
            {info?.error && !info.updateAvailable && (
              <Tooltip title={info.error}>
                <Tag color="default" style={{ marginInlineEnd: 0 }}>
                  {t("plugin.update.unconfirmed")}
                </Tag>
              </Tooltip>
            )}
          </Flex>
        );
      },
    },
    {
      title: t("plugin.col.action"),
      width: 140,
      render: (_, record) => (
        <Space size={4}>
          <Button size="small" disabled={busy} onClick={() => updateOne(record)}>
            {t("plugin.update")}
          </Button>
          <Button
            size="small"
            danger
            type="text"
            disabled={busy}
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
      centered
      footer={<Button onClick={onClose}>{t("plugin.close")}</Button>}
      destroyOnClose
      styles={{
        // 内容超高时在弹窗内部滚动，避免弹窗超出视口、需滚动页面才能看全。
        body: { maxHeight: "calc(100vh - 220px)", overflowY: "auto" },
      }}
    >
      <Space direction="vertical" size={12} style={{ width: "100%" }}>
        {/* profile 选择 */}
        <Flex align="center" gap={8} wrap>
          <Typography.Text type="secondary" style={{ fontSize: 12, whiteSpace: "nowrap" }}>
            profile
          </Typography.Text>
          <Select
            size="small"
            style={{ width: 150 }}
            value={profile}
            disabled={busy}
            options={profileOptions}
            onChange={saveProfile}
            showSearch
            placeholder="web"
          />
          <Tooltip title={t("plugin.profile.tip")}>
            <Typography.Text type="secondary" style={{ fontSize: 12 }}>
              {t("plugin.profile.isolated")}
            </Typography.Text>
          </Tooltip>
        </Flex>

        {/* 操作进度：阶段文字 + 进度条；失败/成功信息在进度条下方展示 */}
        {op && (
          <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
            <Flex align="center" gap={8} wrap>
              <Typography.Text strong style={{ fontSize: 14 }}>
                {op.label}
              </Typography.Text>
              {op.status === "running" ? (
                <Tag color="processing" icon={<LoadingOutlined spin />}>
                  {t("plugin.busy.running")}
                </Tag>
              ) : op.status === "success" ? (
                <Tag color="success">{t("plugin.progress.done")}</Tag>
              ) : (
                <Tag color="error">{t("plugin.progress.error")}</Tag>
              )}
            </Flex>
            <Progress
              size="small"
              status={
                op.status === "running"
                  ? "active"
                  : op.status === "error"
                    ? "exception"
                    : "success"
              }
              percent={op.percent}
            />
            {op.status === "error" && op.message && (
              <Alert
                type="error"
                showIcon
                message={op.message}
                closable
                onClose={() => setOp((prev) => (prev ? { ...prev, message: null } : prev))}
              />
            )}
            {op.status === "success" && op.message && (
              <Alert
                type="success"
                showIcon
                message={op.message}
                closable
                onClose={() => setOp((prev) => (prev ? { ...prev, message: null } : prev))}
              />
            )}
          </div>
        )}

        {/* 安装指引：默认收起为一行，点击向下展开（不弹对话框） */}
        <div
          onClick={() => setGuideOpen((v) => !v)}
          style={{
            display: "flex",
            alignItems: "center",
            gap: 8,
            cursor: "pointer",
            padding: "8px 12px",
            border: `1px solid ${token.colorBorderSecondary}`,
            borderRadius: 8,
            background: token.colorFillQuaternary,
          }}
        >
          {guideOpen ? (
            <DownOutlined style={{ fontSize: 12, color: token.colorTextSecondary }} />
          ) : (
            <RightOutlined style={{ fontSize: 12, color: token.colorTextSecondary }} />
          )}
          <Typography.Text strong style={{ fontSize: 13 }}>
            {guideOpen ? t("plugin.guide.collapse") : t("plugin.guide.expand")}
          </Typography.Text>
        </div>
        {guideOpen && (
          <div
            style={{
              padding: "10px 12px",
              border: `1px solid ${token.colorBorderSecondary}`,
              borderRadius: 8,
              background: token.colorBgContainer,
            }}
          >
            <Typography.Text type="secondary" style={{ fontSize: 13, lineHeight: 1.8 }}>
              {t("plugin.guide.title")}
            </Typography.Text>
            <ul style={{ margin: "6px 0 0", paddingLeft: 18, fontSize: 12, lineHeight: 1.9 }}>
              {(
                [
                  ["plugin.guide.npm.label", "plugin.guide.npm.example"],
                  ["plugin.guide.npmv.label", "plugin.guide.npmv.example"],
                  ["plugin.guide.github.label", "plugin.guide.github.example"],
                  ["plugin.guide.url.label", "plugin.guide.url.example"],
                  ["plugin.guide.cmd.label", "plugin.guide.cmd.example"],
                ] as const
              ).map(([labelKey, exampleKey]) => (
                <li key={labelKey}>
                  <span style={{ opacity: 0.75 }}>{t(labelKey)}：</span>
                  <Typography.Text style={{ fontFamily: "monospace" }}>
                    {t(exampleKey)}
                  </Typography.Text>
                </li>
              ))}
            </ul>
          </div>
        )}

        {/* 安装输入 + 推荐链接 */}
        <Flex gap={8} align="center" wrap>
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
          <Tooltip title={t("plugin.recommend.tip")}>
            <Button
              type="link"
              icon={<LinkOutlined />}
              onClick={() => void openRecommendation()}
            >
              {t("plugin.recommend")}
            </Button>
          </Tooltip>
        </Flex>

        {/* 已安装的第三方插件列表 */}
        <Flex justify="space-between" align="center" gap={8} wrap>
          <Typography.Text strong style={{ fontSize: 13 }}>
            {t("plugin.installed.title")}
          </Typography.Text>
          <Flex align="center" gap={8} wrap>
            {checkingUpdates && <Spin size="small" />}
            {updates && (
              <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                {updatableCount > 0
                  ? t("plugin.updates.found", { 0: updatableCount })
                  : unconfirmedCount > 0
                    ? t("plugin.updates.unconfirmed", { 0: unconfirmedCount })
                    : t("plugin.updates.none")}
              </Typography.Text>
            )}
            <Button
              size="small"
              icon={<CloudSyncOutlined />}
              disabled={busy || checkingUpdates}
              onClick={() => {
                retryAttempt.current = 0;
                void checkUpdates(profile);
              }}
            >
              {t("plugin.checkUpdates")}
            </Button>
          </Flex>
        </Flex>
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
              getCheckboxProps: () => ({ disabled: busy }),
            }}
            locale={{ emptyText: t("plugin.empty") }}
          />
        )}

        {/* 批量操作：更新所选 / 卸载所选 */}
        <Flex justify="space-between" gap={8} wrap>
          <Space>
            <Button
              icon={<CloudSyncOutlined />}
              disabled={busy || selectedKeys.length === 0}
              onClick={updateSelected}
            >
              {selectedKeys.length > 0
                ? t("plugin.updateSelected.n", { 0: selectedKeys.length })
                : t("plugin.updateSelected")}
            </Button>
            <Button
              danger
              disabled={busy || selectedKeys.length === 0}
              onClick={removeSelected}
            >
              {selectedKeys.length > 0
                ? t("plugin.removeSelected", { 0: selectedKeys.length })
                : t("plugin.removeSelectedNone")}
            </Button>
          </Space>
          <Button icon={<ReloadOutlined />} disabled={busy} onClick={() => void loadList(profile)}>
            {t("plugin.refresh")}
          </Button>
        </Flex>
      </Space>
    </Modal>
  );
}
