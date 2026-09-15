// 实时表格共享 Cell：固定布局下内容不推动列宽，数字原地更新不跳动。
import { memo } from "react";
import { Tag } from "antd";
import { formatAge, formatPointValue } from "../deviceModel";
import type { DevicePointView } from "./useDeviceWorkspaceData";
import { PointNameCell } from "./PointNameCell";

/** 等宽数字（HMI/SCADA 风格）：比例字体下 111.11/888.88 同字数不同宽，tabular-nums 固定。 */
export const NUMERIC_MONO = {
  fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace",
  fontVariantNumeric: "tabular-nums",
  fontSize: 12,
} as const;

const ELLIPSIS = {
  whiteSpace: "nowrap",
  overflow: "hidden",
  textOverflow: "ellipsis",
  display: "block",
} as const;

/** 数据点列（P2 双行）：列宽固定，超长省略。 */
export function NameCell({ point }: { point: DevicePointView }) {
  return (
    <span style={ELLIPSIS}>
      <PointNameCell point={point} />
    </span>
  );
}

/** 来源列（P1 标签或 —）：列宽固定，超长省略。 */
export function SourceCell({ point }: { point: DevicePointView }) {
  return (
    <span
      title={point.source_label ? `Driver 来源：${point.source_label}` : "Driver 未提供来源"}
      style={{ ...NUMERIC_MONO, ...ELLIPSIS }}
    >
      {point.sourceText ?? "—"}
    </span>
  );
}

/** 当前值列：等宽 + tabular-nums，数字原地更新不推动布局。 */
export const ValueCell = memo(function ValueCell({ point }: { point: DevicePointView }) {
  return (
    <span title={String(point.value ?? "")} style={{ ...NUMERIC_MONO, ...ELLIPSIS }}>
      {formatPointValue(point.value)}
    </span>
  );
});

/** 状态列：Tag 宽度由文本决定（GOOD/BAD/STALE/UNKNOWN），列宽固定后不挤邻列。 */
export const StatusCell = memo(function StatusCell({ derived }: { derived: DevicePointView["derived"] }) {
  return (
    <Tag color={derived === "GOOD" ? "green" : derived === "BAD" ? "red" : "orange"}>
      {derived}
    </Tag>
  );
});

/**
 * 更新时间列：只依赖原始 timestamp_ns + 父级 tick（秒级），不随
 * devicePoints 整表重建而重建（父组件传稳定 tick，见 useStaleTick）。
 */
export const AgeCell = memo(function AgeCell({ timestampNs, ageMs }: { timestampNs: number; ageMs: number | null }) {
  void timestampNs;
  return <span style={{ ...NUMERIC_MONO, ...ELLIPSIS }}>{formatAge(ageMs)}</span>;
});
