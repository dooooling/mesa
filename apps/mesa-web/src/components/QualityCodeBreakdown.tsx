// PR-D1 质量码分解（计数行上的 Popover，仅点击展开）。
//
// 为什么不在服务端做统计：SSE 已把**全部**点位行推到浏览器，按码聚合是
// O(N) 纯内存计算（238 点微秒级）；新增端点会引入第二份聚合真值、额外鉴权面
// 与"服务端快照 vs 客户端快照"的一致性口径（§2.3）。
//
// 边界（§7）：只有**当前快照**（每点 1 个样本）。Mesa 不保存质量码历史；
// 断线会把该端点全部点的码覆盖为 `COMMUNICATION_LOST`，此前原因不可恢复。
// 因此这里恒显示限制语，且**只点击时**才计算分解（悬停不展开——避免无意间
// 触发大规模统计）。
import { useMemo, useState, type ReactNode } from "react";
import { Button, Popover, Tag, message } from "antd";
import {
  classifyCode,
  codeGroupTitle,
  codeHex,
  namespaceLabel,
  type CodeClass,
} from "../qualityCode";
import { ExportHttpError, buildPointsExport, downloadExportFiles } from "../qualityExport";
import {
  makeQualityScope,
  qualityScopeText,
  type QualityScope,
} from "../qualityScope";

/** 分解只需要这几列：不依赖 DevicePointView 的全部派生字段。 */
export interface BreakdownPoint {
  endpoint_id: string;
  driverId?: string;
  quality: string;
  quality_code?: string;
  value_origin?: string;
}

export interface BreakdownRow {
  endpoint_id: string;
  driver_id: string;
  code: string;
  cls: CodeClass;
  count: number;
}

/** Popover 底部固定限制语（§7.4，逐字）。 */
export const BREAKDOWN_LIMITATION =
  "统计基于当前快照（每点 1 个样本）。Mesa 不保存质量码历史；断线会把该端点全部点的码覆盖为 COMMUNICATION_LOST，此前原因不可恢复。";

/** 分区顺序：Mesa 标记 → 设备/协议返回码 → 未知命名空间。 */
const PARTITION_ORDER: CodeClass[] = [
  "MESA_MARKER",
  "DEVICE_S7_ITEM",
  "PROTOCOL_STATUS",
  "MESA_CLASS",
  "UNKNOWN",
];

/**
 * 按 `(endpoint_id, driver_id, code)` 聚合（§7.2）：跨驱动同名数字**不相加**，
 * 因为同一个 `3` 在 nck 与 focas2 语义不同。无码的行不参与。
 */
export function breakdownByCode(points: BreakdownPoint[]): BreakdownRow[] {
  const acc = new Map<string, BreakdownRow>();
  for (const p of points) {
    if (p.quality_code === undefined) continue;
    const driverId = p.driverId ?? "";
    const key = `${p.endpoint_id}\u0000${driverId}\u0000${p.quality_code}`;
    const existing = acc.get(key);
    if (existing) {
      existing.count += 1;
      continue;
    }
    acc.set(key, {
      endpoint_id: p.endpoint_id,
      driver_id: driverId,
      code: p.quality_code,
      cls: classifyCode(driverId, p.quality_code, p.quality),
      count: 1,
    });
  }
  return [...acc.values()];
}

export function QualityCodeBreakdownContent(props: {
  points: BreakdownPoint[];
  /**
   * 统计/导出分析作用域（设备 + 连接）；`null`/缺省 = 全部设备与全部连接。
   * 与页面计数行/表格共用 qualityScope 的同一真值。
   */
  scope?: QualityScope | null;
  /** 作用域的可读文案（如 `CNC-01 · 全部连接`）；缺省由 scope 推导。 */
  scopeText?: string;
  onExported?: (fileNames: string[]) => void;
}) {
  const { points, scope, scopeText, onExported } = props;
  // 只在内容真正挂载（Popover 打开）时计算一次；关闭后结果随 memo 保留。
  const rows = useMemo(() => breakdownByCode(points), [points]);
  const [exporting, setExporting] = useState(false);
  const effectiveScope = useMemo(
    () => scope ?? makeQualityScope(),
    [scope],
  );
  const currentScopeText = scopeText ?? qualityScopeText(effectiveScope);

  const partitions = useMemo(() => {
    const byTitle = new Map<string, BreakdownRow[]>();
    for (const cls of PARTITION_ORDER) {
      const list = rows.filter((r) => r.cls === cls);
      if (!list.length) continue;
      const title = codeGroupTitle(cls);
      const bucket = byTitle.get(title);
      if (bucket) bucket.push(...list);
      else byTitle.set(title, [...list]);
    }
    return [...byTitle.entries()];
  }, [rows]);

  const runExport = async () => {
    setExporting(true);
    try {
      const result = await buildPointsExport({ scope: effectiveScope });
      downloadExportFiles(result.files);
      onExported?.(result.files.map((f) => f.name));
      message.success(
        result.evidence.raw_preserved === "SEPARATE_FILE"
          ? "已拆分为 2 个文件"
          : "已导出原始快照",
      );
    } catch (e) {
      // 失败即不产文件：文案必须明说"未生成文件"，不留下半份证据。
      const status = e instanceof ExportHttpError ? String(e.status) : "未知";
      message.error(`导出失败：${status}（未生成文件）`);
    } finally {
      setExporting(false);
    }
  };

  return (
    <div style={{ width: 380 }}>
      <div data-testid="breakdown-scope" style={{ fontSize: 12, color: "#525252", marginBottom: 8 }}>
        统计范围：{currentScopeText}（不受质量状态/搜索筛选影响）
      </div>
      {rows.length === 0 ? (
        <div style={{ fontSize: 12, color: "#525252" }}>当前快照中没有质量码</div>
      ) : (
        partitions.map(([title, list]) => (
          <div key={title} data-testid="code-partition" style={{ marginBottom: 8 }}>
            <div style={{ fontWeight: 600, fontSize: 12 }}>{title}</div>
            {list.map((r) => (
              <div
                key={`${r.endpoint_id}\u0000${r.driver_id}\u0000${r.code}`}
                data-testid="code-row"
                style={{ display: "flex", gap: 8, fontSize: 12, alignItems: "center" }}
              >
                <span>{r.endpoint_id}</span>
                <span>{r.driver_id || "未知驱动"}</span>
                <span style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace" }}>
                  {codeHex(r.code) ?? r.code}
                </span>
                <Tag style={{ marginInlineEnd: 0 }}>{namespaceLabel(r.cls) ?? "—"}</Tag>
                <span data-testid="code-count">{r.count}</span>
              </div>
            ))}
          </div>
        ))
      )}
      <div style={{ fontSize: 12, color: "#525252", marginTop: 8 }}>{BREAKDOWN_LIMITATION}</div>
      <div style={{ marginTop: 8 }}>
        <Button size="small" disabled={exporting} onClick={() => void runExport()}>
          {exporting ? "采集中…" : "导出原始快照"}
        </Button>
        {/*
          复审修订 2：raw_scope / analysis_scope 不能只存在于导出文件里。
          用户点击导出前必须在**同一视觉区域**看到范围差异，避免误分享：
          原始证据是全量端点（可能含其他设备），只有结构化分析按所选范围过滤。
        */}
        <div
          data-testid="export-scope-hint"
          style={{ fontSize: 12, color: "#525252", marginTop: 6, lineHeight: 1.6 }}
        >
          <div>原始响应包含全部端点，可能包括其他设备。</div>
          <div>结构化分析仅按所选设备与连接过滤（当前：{currentScopeText}）。</div>
        </div>
      </div>
    </div>
  );
}

/**
 * 计数行包装器：**仅点击**打开分解（悬停不展开），不新增按钮/列，
 * 不改变既有计数文案（children 即原计数行文本）。
 */
export function QualityCodeBreakdown(props: {
  points: BreakdownPoint[];
  scope?: QualityScope | null;
  scopeText?: string;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  return (
    <Popover
      trigger="click"
      open={open}
      onOpenChange={setOpen}
      placement="bottomLeft"
      /*
       * 注意：content **不能**写成 `open ? <Content/> : null`。
       * antd v6 的 Popover 在 content 不可渲染时把 overlay 置空，Tooltip 侧
       * `noTitle` 会强制 setOpen(false) 且不再回调 onOpenChange —— 结果是
       * 点击触发器永远打不开（浏览器里同样如此）。这里始终传元素：
       * rc-trigger 只在首次打开后才挂载 popup，内容组件仍只在展开时 mount，
       * "只在点击时计算一次"的意图不受影响。
       */
      content={
        <QualityCodeBreakdownContent
          points={props.points}
          scope={props.scope}
          scopeText={props.scopeText}
        />
      }
    >
      <span style={{ cursor: "pointer" }}>{props.children}</span>
    </Popover>
  );
}
