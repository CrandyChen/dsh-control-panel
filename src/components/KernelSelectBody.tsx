// 启动内核选择（多版本时）：展示已安装内核列表，携带默认勾选项，并回调用户选择。

import { Radio, Space, Typography } from "antd";
import { useState } from "react";
import { useI18n } from "../i18n";
import type { KernelInstall } from "../types";

interface Props {
  kernels: KernelInstall[];
  defaultValue: string;
  onChange: (id: string) => void;
}

export default function KernelSelectBody({ kernels, defaultValue, onChange }: Props) {
  const { t } = useI18n();
  const [value, setValue] = useState(defaultValue);

  const label = (k: KernelInstall) =>
    k.mode === "prebuilt"
      ? t("start.select.prebuilt", { 0: k.version })
      : t("start.select.source", { 0: k.version });

  return (
    <Radio.Group
      style={{ width: "100%" }}
      value={value}
      onChange={(e) => {
        const next = e.target.value as string;
        setValue(next);
        onChange(next);
      }}
    >
      <Space direction="vertical" style={{ width: "100%" }}>
        {kernels.map((k) => (
          <Radio key={k.id} value={k.id} style={{ width: "100%" }}>
            <Typography.Text style={{ fontSize: 13 }}>{label(k)}</Typography.Text>
          </Radio>
        ))}
      </Space>
    </Radio.Group>
  );
}
