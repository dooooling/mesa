// 「选择实际对象」面板：按需探测 + 展示驱动实际发现的对象 + 选用后填参。
//
// 纪律（每条都对应评审的一条 fail-closed 要求）：
// - **不在挂载时探测**，也不在"选择变量"时探测：只有用户点了「探测实际对象」
//   才发第一个请求（避免每个变量都拉起驱动进程读真机）；
// - **端点未连接不探测**：`gate.available === false` 时按钮禁用 + 原因可见，
//   一个请求都不发（`objectProbe.objectProbeGate`）；
// - **失败必须显示失败**：请求/契约错误渲染成 `object-probe-error`（含码与原文），
//   绝不退化成"空列表"；驱动真的返回 0 项才显示空态，两者文案不同；
// - **不可选/未确认如实显示**：`selection_mode=3`、诊断码（如
//   `IDENTITY_UNRESOLVED` / `DEFINITION_SCOPE_NOT_COLLECTABLE`）逐条展示，
//   `has_children=false` 的节点不提供展开（真机上的"未确认对象"就是这种形态）；
// - **填参只来自驱动载荷**：见 `objectProbe.deriveObjectFill`，失败即拒绝。
import { useEffect, useRef, useState } from "react";
import { Alert, Button, Input, Space, Tag } from "antd";
import { evaluateBrowseNode, type BrowseNode } from "../browseModel";
import {
  canExpandNode,
  deriveObjectFill,
  fetchObjectProbePage,
  isObjectProbeError,
  modeLabel,
  type ObjectFill,
  type ObjectNeed,
  type ObjectProbeGate,
} from "../objectProbe";

export interface ObjectProbePanelProps {
  endpointId: string;
  gate: ObjectProbeGate;
  /** 当前选中的定义（填参依据判定用）。 */
  definition: {
    variable_id?: string;
    revision?: number;
    targets?: unknown;
    target_required?: unknown;
    identity_keys?: unknown;
  } | null;
  /** 需要的对象类型（来自 `resolveObjectNeeds`，已经过数据判定）。 */
  needs: readonly ObjectNeed[];
  /** 选用某个实际对象后的填参结果。 */
  onFill: (fill: ObjectFill) => void;
}

export function ObjectProbePanel({
  endpointId,
  gate,
  definition,
  needs,
  onFill,
}: ObjectProbePanelProps) {
  const [open, setOpen] = useState(false);
  const [parent, setParent] = useState("");
  const [filter, setFilter] = useState("");
  const [nodes, setNodes] = useState<BrowseNode[]>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  // 失败状态与"空页"严格分开：null ≠ 请求失败。
  const [error, setError] = useState<{ code: string; message: string } | null>(null);
  const [fillError, setFillError] = useState<{ code: string; message: string } | null>(null);
  const [applied, setApplied] = useState<ObjectFill | null>(null);
  // 只采纳最后一次请求的结果（快速连点"进入"时旧响应不得覆盖新页）。
  const token = useRef(0);

  const load = (nextParent: string, nextCursor: string | null, append: boolean) => {
    if (!gate.available) return;
    const seq = ++token.current;
    setLoading(true);
    setError(null);
    void fetchObjectProbePage({
      endpointId,
      parent: nextParent,
      filter,
      cursor: nextCursor ?? "",
    })
      .then((page) => {
        if (seq !== token.current) return;
        setNodes((prev) => (append ? [...prev, ...page.nodes] : page.nodes));
        setCursor(page.nextCursor);
      })
      .catch((e: unknown) => {
        if (seq !== token.current) return;
        // 失败时**清空**列表并保留错误：不允许"失败看起来像没有对象"。
        setNodes([]);
        setCursor(null);
        setError(
          isObjectProbeError(e)
            ? { code: e.code, message: e.message }
            : { code: "PROBE_REQUEST_FAILED", message: e instanceof Error ? e.message : String(e) },
        );
      })
      .finally(() => {
        if (seq !== token.current) return;
        setLoading(false);
      });
  };

  // 变量一换，上一次的对象探测结论不再适用（不把别的变量的对象带进来）。
  const definitionKey = `${String(definition?.variable_id ?? "")}@${String(
    definition?.revision ?? "",
  )}`;
  useEffect(() => {
    setApplied(null);
    setFillError(null);
    if (!open) return;
    setParent("");
    setNodes([]);
    setCursor(null);
    load("", null, false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [definitionKey]);

  const start = () => {
    setOpen(true);
    setParent("");
    setNodes([]);
    setCursor(null);
    setApplied(null);
    setFillError(null);
    load("", null, false);
  };

  const enter = (id: string) => {
    setParent(id);
    setNodes([]);
    setCursor(null);
    load(id, null, false);
  };

  const use = (node: BrowseNode) => {
    const result = deriveObjectFill({ node, definition, needs });
    if (!result.ok) {
      setApplied(null);
      setFillError({ code: result.code, message: result.message });
      return;
    }
    setFillError(null);
    setApplied(result.fill);
    onFill(result.fill);
  };

  const needText = needs.length
    ? `需要选择：${needs.map((need) => `${need.kind}${need.required ? "*" : ""}`).join(" / ")}`
    : "该变量不需要额外对象";

  return (
    <div data-testid="object-probe" style={{ display: "grid", gap: 8 }}>
      <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
        <span style={{ fontSize: 12 }}>实际对象</span>
        <Button
          size="small"
          data-testid="object-probe-open"
          disabled={!gate.available || loading}
          onClick={start}
        >
          探测实际对象
        </Button>
        <span style={{ fontSize: 11, color: "#525252" }} data-testid="object-probe-needs">
          {needText}
        </span>
      </div>

      {/* 端点未连接：只说原因，不发请求（评审硬要求）。 */}
      {!gate.available ? (
        <div data-testid="object-probe-gate" style={{ fontSize: 11, color: "#c62828" }}>
          {gate.reason}
        </div>
      ) : null}

      {open && gate.available ? (
        <div
          data-testid="object-probe-panel"
          style={{ display: "grid", gap: 6, border: "1px solid #e0e0e0", padding: 8 }}
        >
          <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
            <span style={{ fontSize: 11, color: "#525252" }}>位置</span>
            <code data-testid="object-probe-path" style={{ fontSize: 11 }}>
              {parent || "（根）"}
            </code>
            {parent ? (
              <Button size="small" data-testid="object-probe-root" onClick={() => enter("")}>
                回根
              </Button>
            ) : null}
            <Input
              size="small"
              data-testid="object-probe-filter"
              placeholder="过滤（交给驱动）"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              style={{ width: 160 }}
              allowClear
            />
            <Button
              size="small"
              data-testid="object-probe-refresh"
              loading={loading}
              onClick={() => load(parent, null, false)}
            >
              重新探测
            </Button>
          </div>

          {/* 失败优先于一切：不得渲染成"空"（评审要求）。 */}
          {error ? (
            <div data-testid="object-probe-error" data-code={error.code} style={{ fontSize: 12, color: "#c62828" }}>
              探测失败（{error.code}）：{error.message}
            </div>
          ) : null}

          {!error && !loading && nodes.length === 0 ? (
            <div data-testid="object-probe-empty" style={{ fontSize: 11, color: "#525252" }}>
              驱动在该位置没有返回任何对象（这是探测结果，不是失败）。
            </div>
          ) : null}

          {nodes.map((node) => {
            const evaluation = evaluateBrowseNode(node);
            const expandable = canExpandNode(node);
            return (
              <div
                key={node.id}
                data-testid={`object-probe-node-${node.id}`}
                data-mode={evaluation.mode}
                style={{ display: "flex", gap: 8, alignItems: "flex-start", borderTop: "1px dashed #eee", paddingTop: 4 }}
              >
                <span style={{ flex: 1, minWidth: 0 }}>
                  <span style={{ fontSize: 12 }}>{node.label || node.id}</span>
                  <code style={{ display: "block", fontSize: 10, color: "#8c8c8c", overflowWrap: "anywhere" }}>
                    {node.id}
                  </code>
                  {/* 文档化：诊断码与文案原样展示（真机上就是它们说明"为什么不能选"）。 */}
                  {(node.diagnostics ?? []).map((diagnostic, index) => (
                    <span
                      key={`${diagnostic.code}-${index}`}
                      data-testid={`object-probe-diagnostic-${diagnostic.code}`}
                      style={{
                        display: "block",
                        fontSize: 10,
                        color: (diagnostic.severity ?? 0) >= 3 ? "#c62828" : "#8c6000",
                      }}
                    >
                      {diagnostic.code}（severity {String(diagnostic.severity ?? 0)}）：{diagnostic.message}
                    </span>
                  ))}
                  {evaluation.blocks.map((block) => (
                    <span
                      key={`${block.code}-${block.message}`}
                      data-testid={`object-probe-block-${block.code}`}
                      style={{ display: "block", fontSize: 10, color: "#c62828" }}
                    >
                      不可选用（{block.code}）：{block.message}
                    </span>
                  ))}
                </span>
                <Tag style={{ fontSize: 10, marginInlineEnd: 0 }}>{modeLabel(evaluation.mode)}</Tag>
                {node.kind ? <Tag style={{ fontSize: 10, marginInlineEnd: 0 }}>{node.kind}</Tag> : null}
                <Space size={4}>
                  {expandable ? (
                    <Button size="small" data-testid={`object-probe-expand-${node.id}`} onClick={() => enter(node.id)}>
                      进入
                    </Button>
                  ) : (
                    <span data-testid={`object-probe-nochildren-${node.id}`} style={{ fontSize: 10, color: "#8c8c8c" }}>
                      无子节点
                    </span>
                  )}
                  {evaluation.semanticPayload ? (
                    <Button size="small" type="primary" data-testid={`object-probe-use-${node.id}`} onClick={() => use(node)}>
                      用这个对象
                    </Button>
                  ) : null}
                </Space>
              </div>
            );
          })}

          {cursor ? (
            <Button size="small" data-testid="object-probe-next" loading={loading} onClick={() => load(parent, cursor, true)}>
              下一页
            </Button>
          ) : null}

          {applied ? (
            <div data-testid="object-probe-applied" style={{ fontSize: 11, color: "#2e7d32" }}>
              已填参：{Object.entries(applied.target).map(([key, value]) => `${key}=${value}`).join(" · ") || "（无 target）"}
              {applied.unresolved.length
                ? ` · 仍需手动填写：${applied.unresolved.join(" / ")}`
                : ""}
              {" — "}
              {applied.basis}
            </div>
          ) : null}
          {fillError ? (
            <div data-testid="object-probe-fill-error" data-code={fillError.code} style={{ fontSize: 11, color: "#c62828" }}>
              不可自动填参（{fillError.code}）：{fillError.message}
            </div>
          ) : null}
        </div>
      ) : null}

      {open && gate.available && error ? (
        <Alert
          type="error"
          showIcon
          data-testid="object-probe-error-alert"
          message="对象探测失败：不会用空列表冒充结果"
          description={`${error.code}：${error.message}`}
        />
      ) : null}
    </div>
  );
}
