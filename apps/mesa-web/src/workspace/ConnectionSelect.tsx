// 连接筛选器：纯展示组件，不知道自己在哪个 Tab。
// allowAll=true（Data/Events）：[全部连接 ▼]；false（Acquisition/Diagnostics）：单选。
// 超过 3~4 个连接时 Select 比按钮墙更符合过滤器语义。
import { Select } from "antd";
import type { WorkspaceEndpoint } from "./useDeviceWorkspaceData";

export function ConnectionSelect(props: {
  endpoints: WorkspaceEndpoint[];
  /** null = 全部（仅 allowAll 时合法）。 */
  value: string | null;
  allowAll?: boolean;
  onChange: (id: string | null) => void;
  width?: number;
}) {
  const { endpoints, value, allowAll = true, onChange, width = 180 } = props;
  return (
    <span style={{ fontSize: 12 }}>
      连接{" "}
      <Select
        value={allowAll ? (value ?? "ALL") : (value ?? endpoints[0]?.id ?? null)}
        onChange={(v) => onChange(v === "ALL" ? null : (v as string))}
        style={{ width }}
        options={[
          ...(allowAll ? [{ value: "ALL", label: "全部连接" }] : []),
          ...endpoints.map((e) => ({ value: e.id, label: e.name ?? e.id })),
        ]}
      />
    </span>
  );
}
