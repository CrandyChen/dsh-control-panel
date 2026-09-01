// 内嵌浏览器 Tab 栏（纯前端）：固定「主界面」主页 + 可切换/关闭的标签页。
// 标签页由程序自动添加（如打开 DSH 界面、打开安装指引），不允许用户手动新建。

import { CloseOutlined, HomeOutlined } from "@ant-design/icons";
import { Button, Flex, theme, Typography } from "antd";
import { TAB_BAR_HEIGHT } from "../constants";
import { useI18n } from "../i18n";
import type { BrowserTab } from "../types";

interface Props {
  tabs: BrowserTab[];
  activeTabId: string | null;
  onSelect: (id: string) => void;
  onClose: (id: string) => void;
  onShowHome: () => void;
}

export default function WebTabBar({
  tabs,
  activeTabId,
  onSelect,
  onClose,
  onShowHome,
}: Props) {
  const { token } = theme.useToken();
  const { t } = useI18n();

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
    </Flex>
  );
}
