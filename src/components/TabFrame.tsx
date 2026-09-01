// 单个浏览器 Tab 的容器：普通 Tab 用 iframe 承载；DSH 界面（native）由程序内
// 原生子 Webview 呈现（覆盖于锚点区域之上），这里只保留加载占位。

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
      {tab.native ? (
        // DSH 界面由程序内**原生子 Webview** 呈现（覆盖在锚点区域之上），
        // 这里不渲染 iframe（会 401），仅保留一个占位（原生 Webview 挂载前短暂可见加载态）。
        <div style={{ position: "absolute", inset: 0 }} />
      ) : (
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
      )}
    </div>
  );
}
