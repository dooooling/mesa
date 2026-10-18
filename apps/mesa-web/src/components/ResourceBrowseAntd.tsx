// Descriptor-driven Browse 选型（P0-2）：消费 resource_selection_methods
// 声明的 Browse 能力。只理解 browse 节点信封（见 browseModel），不理解
// 任何协议：下钻靠 has_children + 节点 id 回传 parent；回根即 parent=""；
// 选用须经 selectableFromNode 判定，无 binding 的分支只能进入。
//
// PR-C 第三批：语义叶（nck_variable / nck_native）不再走通用“resource 回填”
// 路径，而是：
// - 语义叶必须带 identity（A16），否则入口关闭；
// - `scope=identity` / 缺定义 / family 不一致 / Browse 预览为空 ⇒ 显示原因且
//   不给“选用”（A13）；
// - 可选用时交给父层的语义表单（服务端 Preview 仍是唯一身份来源），
//   **绝不**把 Browse 的 preview_point_key 直接当提交 Key；
// - 非 NCK 叶逐字保持既有行为（A44）。
import { useEffect, useState } from "react";
import { Button, Input, Space, Tag, message } from "antd";
import {
  buildBrowseRequest,
  selectableFromNode,
  type BrowseNode,
  type BrowsePointKind,
  type BrowseSelection,
} from "../browseModel";
import { evaluateNckSelectionAdd } from "../nckCollectability";
import {
  checkNckFamily,
  type NckDefinitionLike,
} from "../nckPointModel";

/** Browse 的语义准入上下文（由 Picker / Endpoint 页注入）。 */
export interface NckSemanticBrowseConfig {
  /** 目录定义（只读）。缺失即语义叶一律不可选（fail-closed）。 */
  definitions?: readonly NckDefinitionLike[] | null;
  endpointFamily?: string | null;
  catalogFamily?: string | null;
}

interface SemanticGate {
  ok: boolean;
  reason?: string;
}

function findDefinition(
  definitions: readonly NckDefinitionLike[] | null | undefined,
  variableId: string,
  revision: number | null,
): NckDefinitionLike | null {
  if (!definitions) return null;
  for (const definition of definitions) {
    if (definition.variable_id !== variableId) continue;
    if (
      revision !== null &&
      typeof definition.revision === "number" &&
      definition.revision !== revision
    ) {
      continue;
    }
    return definition;
  }
  return null;
}

/** 解析原始信封里的语义 kind（仅用于“为什么不可选”的展示）。 */
function rawSemanticKind(node: BrowseNode): BrowsePointKind | null {
  try {
    const parsed = JSON.parse((node.binding_json ?? "").trim()) as { kind?: unknown };
    return parsed?.kind === "nck_variable" || parsed?.kind === "nck_native"
      ? parsed.kind
      : null;
  } catch {
    return null;
  }
}

/** 旧 `variable` 原生信封（语义载荷不可用时的降级形态）。 */
function isLegacyVariableEnvelope(node: BrowseNode): boolean {
  try {
    const parsed = JSON.parse((node.binding_json ?? "").trim()) as {
      kind?: unknown;
      resource_id?: unknown;
    };
    return parsed?.resource_id === "variable" && parsed?.kind === undefined;
  } catch {
    return false;
  }
}

/**
 * 语义叶准入（A13/A16）：目录、scope、family。
 *
 * 这里**不**判断 Browse 预览 Key 的权威性——它只用于展示，提交前语义表单会
 * 重新走服务端 identity-preview。
 */
export function evaluateSemanticBrowseLeaf(
  selection: BrowseSelection,
  config: NckSemanticBrowseConfig | null | undefined,
): SemanticGate {
  if (!config) return { ok: false, reason: "未提供语义目录上下文：不可选用" };
  if (selection.point_kind === "nck_native") {
    return { ok: false, reason: "原生点位请使用「高级原生配置」，不要从语义 Browse 加入" };
  }
  if (selection.point_kind !== "nck_variable") {
    return { ok: false, reason: "非语义叶：走通用资源回填" };
  }
  const variableId = selection.parameters.variable_id;
  const revision = selection.parameters.definition_revision;
  if (typeof variableId !== "string" || variableId === "") {
    return { ok: false, reason: "语义叶缺少 variable_id" };
  }
  if (typeof revision !== "number" || !Number.isInteger(revision)) {
    return { ok: false, reason: "语义叶缺少 definition_revision" };
  }
  if (!config.definitions) {
    return { ok: false, reason: "定义目录不可用：无法确认 scope，禁止选用（fail-closed）" };
  }
  const definition = findDefinition(config.definitions, variableId, revision);
  if (!definition) {
    return {
      ok: false,
      reason: `定义目录中找不到 ${variableId}（rev ${revision}）：无法确认 scope`,
    };
  }
  const admission = evaluateNckSelectionAdd(definition);
  if (!admission.ok) {
    return { ok: false, reason: `${admission.code}：${admission.message}` };
  }
  const family = checkNckFamily({
    endpointFamily: config.endpointFamily,
    definitionFamily: definition.family,
    catalogFamily: config.catalogFamily,
  });
  if (!family.ok) return { ok: false, reason: `${family.code}：${family.message}` };
  // `preview_point_key=null` 只表示该模板的 target/arguments 尚未补全（A10）：
  // 仍允许进入语义表单补全，但**提交**必须等服务端 Preview 成功。
  // 因此这里不拒绝，只在 UI 上显示“身份未确定”。
  return { ok: true };
}

export function ResourceBrowseAntd({
  endpointId,
  onFill,
  nckSemantic,
  onSemanticSelect,
}: {
  endpointId: string;
  /** 用户选用通用资源叶：父层切到手动表单并预填（resource + parameters）。 */
  onFill: (sel: BrowseSelection) => void;
  /** 语义准入上下文；未注入时语义叶一律不可选（fail-closed）。 */
  nckSemantic?: NckSemanticBrowseConfig | null;
  /**
   * 用户选用语义叶：父层应打开语义表单（NckPointFields）并预填 parameters。
   * 未提供时语义叶“选用”入口关闭（不回落到旧 resource 路径）。
   */
  onSemanticSelect?: (sel: BrowseSelection) => void;
}) {
  const [parent, setParent] = useState("");
  const [filter, setFilter] = useState("");
  const [nodes, setNodes] = useState<BrowseNode[]>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [failed, setFailed] = useState(false);

  const load = async (p: string, f: string, c: string | null, append: boolean) => {
    setLoading(true);
    setFailed(false);
    try {
      const r = await fetch(`/api/v1/endpoints/${endpointId}/browse`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(buildBrowseRequest({ parent: p, filter: f, cursor: c ?? "" })),
      });
      const j = await r.json().catch(() => ({}));
      if (!r.ok) {
        message.error(j.error?.message ?? "浏览失败");
        setFailed(true);
        return;
      }
      const page = (j.nodes ?? []) as BrowseNode[];
      setNodes((prev) => (append ? [...prev, ...page] : page));
      setCursor(j.next_cursor ?? null);
    } catch {
      message.error("浏览请求失败");
      setFailed(true);
    } finally {
      setLoading(false);
    }
  };

  // endpoint 切换时回到根
  useEffect(() => {
    setParent("");
    setFilter("");
    setCursor(null);
    setNodes([]);
    load("", "", null, false);
  }, [endpointId]); // eslint-disable-line react-hooks/exhaustive-deps

  const enter = (id: string) => {
    setParent(id);
    setCursor(null);
    load(id, filter, null, false);
  };

  const backToRoot = () => {
    setParent("");
    setCursor(null);
    load("", filter, null, false);
  };

  const search = () => {
    setCursor(null);
    load(parent, filter, null, false);
  };

  return (
    <div style={{ display: "grid", gap: 12 }}>
      <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
        <span style={{ fontSize: 12, color: "#525252" }}>位置</span>
        <span style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 12 }}>
          {parent || "（根）"}
        </span>
        {!!parent && (
          <Button size="small" onClick={backToRoot}>回根</Button>
        )}
        <Input
          placeholder="过滤"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          onPressEnter={search}
          style={{ width: 200 }}
          allowClear
        />
        <Button size="small" type="primary" onClick={search} loading={loading}>浏览</Button>
      </div>

      {failed && !nodes.length ? (
        <div style={{ fontSize: 12, color: "#525252" }}>浏览不可用（驱动可能不支持 Browse，或连接不可达）。</div>
      ) : (
        <div style={{ display: "grid", gap: 6 }}>
          {nodes.map((n) => {
            const sel = selectableFromNode(n);
            const rawKind = rawSemanticKind(n);
            const semantic = sel?.point_kind ?? rawKind;
            // NCK 语义上下文里，旧 `variable` 原生信封是“语义载荷不可用”的
            // 降级形态（presets.rs:195-203）：只提示改用高级原生配置，
            // **不得**回落到旧通用 resource 回填（A22）。
            const legacyVariable = !!nckSemantic && !semantic && isLegacyVariableEnvelope(n);
            let gate: SemanticGate | null = null;
            if (semantic) {
              gate = sel
                ? evaluateSemanticBrowseLeaf(sel, nckSemantic)
                : { ok: false, reason: "语义叶缺少 identity 或形态非法：不可选用（A16）" };
              if (gate.ok && !onSemanticSelect) {
                gate = { ok: false, reason: "上层未提供语义表单入口：不可选用" };
              }
            }
            return (
              <div
                key={n.id}
                style={{ display: "flex", gap: 8, alignItems: "center", padding: "8px 10px", border: "1px solid #e0e0e0" }}
              >
                <span style={{ flex: 1, minWidth: 0 }}>
                  <span style={{ fontSize: 13 }}>{n.label || n.id}</span>
                  <span style={{ display: "block", fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 11, color: "#525252", overflow: "hidden", textOverflow: "ellipsis" }}>
                    {n.id}
                  </span>
                  {semantic ? (
                    <span style={{ display: "block", fontSize: 11, color: "#525252" }}>
                      <Tag color="blue">语义点位</Tag>
                      {semantic === "nck_native" ? <Tag>nck_native</Tag> : null}
                      {sel?.identity?.preview_point_key ? (
                        <span style={{ fontFamily: "'IBM Plex Mono',monospace" }}>
                          Browse 预览：{sel.identity.preview_point_key}（仅展示；提交前仍需服务端确认）
                        </span>
                      ) : semantic === "nck_variable" && sel?.identity ? (
                        <span style={{ color: "#8c6000" }}>
                          Browse 预览：身份未确定（target/arguments 不完整）——可选用后到语义表单补全，
                          提交前必须由服务端 Preview 确认
                        </span>
                      ) : null}
                    </span>
                  ) : null}
                  {legacyVariable ? (
                    <span
                      data-testid="browse-legacy-variable-block"
                      style={{ display: "block", fontSize: 11, color: "#c62828" }}
                    >
                      语义载荷不可用（内嵌定义目录缺少该变量）：只能使用「高级原生配置」，
                      不得走旧 variable 采集入口
                    </span>
                  ) : null}
                  {gate && !gate.ok ? (
                    <span
                      data-testid="browse-semantic-block"
                      style={{ display: "block", fontSize: 11, color: "#c62828" }}
                    >
                      不可选用：{gate.reason}
                    </span>
                  ) : null}
                </span>
                {n.kind ? <Tag>{n.kind}</Tag> : null}
                {n.data_type ? <Tag>{n.data_type}</Tag> : null}
                {n.access && n.access !== "read" ? <Tag color="orange">{n.access}</Tag> : null}
                <Space>
                  {n.has_children ? (
                    <Button size="small" onClick={() => enter(n.id)}>进入</Button>
                  ) : null}
                  {semantic ? (
                    gate?.ok && sel ? (
                      <Button
                        size="small"
                        type="primary"
                        onClick={() => onSemanticSelect?.(sel)}
                      >
                        选用（语义）
                      </Button>
                    ) : null
                  ) : sel && !legacyVariable ? (
                    <Button size="small" type="primary" onClick={() => onFill(sel)}>选用</Button>
                  ) : null}
                </Space>
              </div>
            );
          })}
          {!nodes.length && !loading && (
            <div style={{ fontSize: 12, color: "#525252" }}>空（未知 parent 返回空页是正常探索语义，可回根）</div>
          )}
        </div>
      )}

      {!!cursor && (
        <Button onClick={() => load(parent, filter, cursor, true)} loading={loading}>下一页</Button>
      )}
    </div>
  );
}
