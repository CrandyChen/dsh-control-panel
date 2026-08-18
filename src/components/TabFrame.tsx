// 单个浏览器 Tab 的 iframe 容器：加载完成前显示「正在加载」提示。

import { LoadingOutlined } from "@ant-design/icons";
import { Flex, Spin, theme, Typography } from "antd";
import { useState } from "react";
import { useI18n } from "../i18n";
import type { BrowserTab } from "../types";

interface Props {
  tab: BrowserTab;
  active: boolean;
}

export default function TabFrame({ tab, active }: Props) {
  const { token } = theme.useToken();
  const { t } = useI18n();
  const [loaded, setLoaded] = useState(false);

  return (
    <div
      style={{
        position: "absolute",
        inset: 0,
        display: active ? "block" : "none",
      }}
    >
      {!loaded && (
        <Flex
          align="center"
          justify="center"
          style={{ position: "absolute", inset: 0, background: token.colorBgLayout, zIndex: 1 }}
        >
          <Spin indicator={<LoadingOutlined spin />} size="large" />
          <Typography.Text type="secondary" style={{ marginLeft: 12 }}>
            {t("tab.loading", { 0: tab.title })}
          </Typography.Text>
        </Flex>
      )}
      <iframe
        src={tab.url}
        title={tab.title}
        onLoad={() => setLoaded(true)}
        style={{
          position: "absolute",
          inset: 0,
          width: "100%",
          height: "100%",
          border: "none",
        }}
      />
    </div>
  );
}
