// 内嵌浏览器 Tab 栏（纯前端）：固定「主界面」主页 + 可切换/关闭的标签页 + 新建。

import { CloseOutlined, HomeOutlined, PlusOutlined } from "@ant-design/icons";
import { App, Button, Flex, Input, Modal, theme, Tooltip, Typography } from "antd";
import { useState } from "react";
import { TAB_BAR_HEIGHT, WEB_URL } from "../constants";
import { useI18n } from "../i18n";
import type { BrowserTab } from "../types";

interface Props {
  tabs: BrowserTab[];
  activeTabId: string | null;
  onSelect: (id: string) => void;
  onClose: (id: string) => void;
  onNew: (url: string) => void;
  onShowHome: () => void;
}

export default function WebTabBar({
  tabs,
  activeTabId,
  onSelect,
  onClose,
  onNew,
  onShowHome,
}: Props) {
  const { token } = theme.useToken();
  const { message } = App.useApp();
  const { t } = useI18n();
  const [newOpen, setNewOpen] = useState(false);
  const [url, setUrl] = useState(WEB_URL);

  const confirmNew = () => {
    const u = url.trim();
    if (!/^https?:\/\/.+/.test(u)) {
      message.warning(t("tab.url.invalid"));
      return;
    }
    onNew(u);
    setNewOpen(false);
    setUrl(WEB_URL);
  };

  const homeActive = activeTabId === null;

  const tabStyle = (active: boolean): React.CSSProperties => ({
    height: 30,
    padding: "0 4px 0 12px",
    borderRadius: 6,
    cursor: "pointer",
    background: active ? token.colorPrimary : token.colorFillTertiary,
    color: active ? token.colorTextLightSolid : token.colorText,
    maxWidth: 220,
    display: "inline-flex",
    alignItems: "center",
    gap: 4,
    flexShrink: 0,
  });

  return (
    <>
      <Flex
        align="center"
        gap={6}
        style={{
          height: TAB_BAR_HEIGHT,
          padding: "0 12px",
          borderBottom: `1px solid ${token.colorBorderSecondary}`,
          background: token.colorBgContainer,
          overflowX: "auto",
          whiteSpace: "nowrap",
          flexShrink: 0,
        }}
      >
        {/* 固定的「主界面」主页：默认界面永不关闭 */}
        <Flex align="center" gap={4} style={tabStyle(homeActive)} onClick={onShowHome}>
          <HomeOutlined style={{ fontSize: 12 }} />
          <Typography.Text
            style={{ fontSize: 12, color: "inherit", overflow: "hidden", textOverflow: "ellipsis" }}
          >
            {t("tab.home")}
          </Typography.Text>
        </Flex>

        {tabs.map((tab) => {
          const active = tab.id === activeTabId;
          return (
            <Flex
              key={tab.id}
              align="center"
              gap={4}
              style={tabStyle(active)}
              onClick={() => onSelect(tab.id)}
            >
              <Typography.Text
                style={{
                  fontSize: 12,
                  color: "inherit",
                  maxWidth: 150,
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                }}
                title={tab.url}
              >
                {tab.title}
              </Typography.Text>
              <Button
                size="small"
                type="text"
                icon={<CloseOutlined style={{ fontSize: 10 }} />}
                style={{
                  color: "inherit",
                  width: 20,
                  height: 20,
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "center",
                }}
                onClick={(e) => {
                  e.stopPropagation();
                  onClose(tab.id);
                }}
              />
            </Flex>
          );
        })}
        <Tooltip title={t("tab.new")}>
          <Button
            size="small"
            type="text"
            icon={<PlusOutlined />}
            onClick={() => setNewOpen(true)}
          />
        </Tooltip>
      </Flex>

      <Modal
        title={t("tab.new.title")}
        open={newOpen}
        onOk={confirmNew}
        onCancel={() => setNewOpen(false)}
        okText={t("tab.new.open")}
        cancelText={t("tab.new.cancel")}
        width={520}
      >
        <Flex gap={8} align="center">
          <Input
            placeholder="http://127.0.0.1:3080"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            onPressEnter={confirmNew}
          />
        </Flex>
        <Typography.Text type="secondary" style={{ fontSize: 12 }}>
          {t("tab.new.hint")}
        </Typography.Text>
      </Modal>
    </>
  );
}
