// 实时表格共享 Cell：固定布局下内容不推动列宽，数字原地更新不跳动。
// 全部只收自己需要的 primitive props：timestamp 变化不带动 Name/Source/Value
// 跟着 render（memo 身份稳定才真正 bailout）。
import { memo } from "react";
import { Tag } from "antd";
import { formatAge, pointAgeMs } from "../deviceModel";
import { PointNameCell } from "./PointNameCell";
import { useStaleTick } from "./StaleClock";

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
export const NameCell = memo(function NameCell({
  displayKey,
  pointKey,
  displayName,
}: {
  displayKey: string;
  pointKey: string;
  displayName?: string;
}) {
  return (
    <span style={ELLIPSIS}>
      <PointNameCell displayKey={displayKey} pointKey={pointKey} displayName={displayName} />
    </span>
  );
});

/** 来源列（P1 标签或 —）：列宽固定，超长省略。 */
export const SourceCell = memo(function SourceCell({
  sourceText,
  sourceLabel,
}: {
  sourceText: string | null;
  sourceLabel?: string;
}) {
  return (
    <span
      title={sourceLabel ? `Driver 来源：${sourceLabel}` : "Driver 未提供来源"}
      style={{ ...NUMERIC_MONO, ...ELLIPSIS }}
    >
      {sourceText ?? "—"}
    </span>
  );
});

/** 当前值列：等宽 + tabular-nums，数字原地更新不推动布局。 */
export const ValueCell = memo(
  function ValueCell({ value }: { value: unknown }) {
    return (
      <span title={String(value ?? "")} style={{ ...NUMERIC_MONO, ...ELLIPSIS }}>
        {formatValue(value)}
      </span>
    );
  },
  (a, b) => valueEqual(a.value, b.value),
);

/** 值相等：primitive 用 Object.is；对象/数组走 JSON（fetch 每次新引用但内容可同）。 */
function valueEqual(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (typeof a !== typeof b) return false;
  if (a === null || b === null) return a === b;
  if (typeof a === "object") {
    try {
      return JSON.stringify(a) === JSON.stringify(b);
    } catch {
      return false;
    }
  }
  return false;
}

/** 值渲染（与 deviceModel.formatPointValue 同行为：标量直显，超长截断 160）。 */
function formatValue(value: unknown): string {
  if (value === null || value === undefined) return "—";
  if (typeof value === "string") return value.length > 160 ? `${value.slice(0, 160)}…` : value;
  if (typeof value === "number" || typeof value === "boolean" || typeof value === "bigint") {
    return String(value);
  }
  try {
    const s = JSON.stringify(value) ?? String(value);
    return s.length > 160 ? `${s.slice(0, 160)}…` : s;
  } catch {
    return String(value);
  }
}

/** 状态列：Tag 宽度由文本决定（GOOD/BAD/STALE/UNKNOWN），列宽固定后不挤邻列。 */
export const StatusCell = memo(function StatusCell({ derived }: { derived: string }) {
  return (
    <Tag color={derived === "GOOD" ? "green" : derived === "BAD" ? "red" : "orange"}>
      {derived}
    </Tag>
  );
});

/**
 * 更新时间列：只收 timestamp_ns，年龄由共享秒钟现算。
 * tick 每秒变，但本组件是叶子 span，重渲染成本即一个文本节点。
 */
export function AgeCell({ timestampNs }: { timestampNs: number }) {
  const tick = useStaleTick();
  void tick;
  const age = pointAgeMs(timestampNs, Date.now());
  return <span style={{ ...NUMERIC_MONO, ...ELLIPSIS }}>{formatAge(age)}</span>;
}
