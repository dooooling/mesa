// Descriptor 驱动的 Browse 选型（P0-2）：消费 resource_selection_methods 声明的
// Browse 能力。
//
// 本组件只做三件事，且**只**理解 wire 契约（见 browseModel.ts）：
//
// 1. 渲染状态与诊断：`evaluateBrowseNode(node)` 给出「可选（资源/语义）/
//    阻断 / 无入口」三态 + 可见阻断原因 + 展示性提示；组件原样渲染，
//    **不解释任何取值**（不认识厂商、协议、point kind）；
// 2. 把**不透明载荷**派发出去：资源叶 → `onFill`；语义叶 → 注入的
//    `BrowseSemanticHandler.parse` 解析后 → `onSemanticSelect`；
// 3. 下钻：`has_children` + 节点 id 回传 parent；回根即 parent=""。
//
// 可选性与阻断原因由**驱动**在 wire 上给出（IPC 1.8 `selection_mode` /
// `selection_payload_json` / `diagnostics`）。旧驱动（未声明）走既有通用规则，
// 语义载荷因缺少权威准入结论而一律不可选。
//
// fail-closed（缺一即为回归，任何一条都必须是"可见阻断"而不是静默回退）：
// - 未知选择模式 / 载荷缺失 / 载荷畸形 / 资源载荷不合契约 ⇒ 阻断；
// - 语义叶但没有处理器 / 上层未接线 ⇒ 阻断（绝不回落到资源回填）；
// - 处理器拒绝载荷 ⇒ 阻断（码与文案由语义层给出）；
// - 驱动声明 NOT_SELECTABLE ⇒ 永不给入口，哪怕载荷看起来合法。
//
// 身份纪律（与驱动无关的通用要求）：节点自带的预览身份**只展示**，永不作为
// 最终身份；提交前必须由服务端 Preview / Configure 重新确认。
import { useEffect, useState } from "react";
import { Button, Input, Space, Tag, message } from "antd";
import {
  buildBrowseRequest,
  evaluateBrowseNode,
  type BrowseBlock,
  type BrowseNode,
  type BrowseSelection,
} from "../browseModel";
import {
  DEFAULT_MISSING_HANDLER_REASON,
  missingHandlerBlock,
  parseRejectedBlock,
  type BrowseSemanticHandler,
} from "../browseSemanticHandler";

/** 单个节点的渲染视图：判定结果 + 已解析的语义载荷（若有）。 */
interface LeafView {
  resource: BrowseSelection | null;
  notes: readonly { code: string; message: string; tone: "muted" | "warn" }[];
  blocks: readonly BrowseBlock[];
  /** 语义入口是否成立（处理器解析成功且上层已接线）。 */
  semantic: { selection: unknown; label: string } | null;
}

export function ResourceBrowseAntd<TSel>({
  endpointId,
  onFill,
  semanticHandler,
  onSemanticSelect,
}: {
  endpointId: string;
  /** 用户选用通用资源叶：父层切到手动表单并预填（resource + parameters）。 */
  onFill: (sel: BrowseSelection) => void;
  /**
   * 语义载荷处理器（可选）。由驱动语义层注入；未注入时所有 SEMANTIC 叶
   * 一律不可选用（fail-closed），资源叶与旧驱动行为逐字不变。
   */
  semanticHandler?: BrowseSemanticHandler<TSel> | null;
  /**
   * 用户选用语义叶：父层按载荷（不透明）走自己的表单。
   * 未提供时语义叶的「选用」入口关闭（不回落到资源回填）。
   */
  onSemanticSelect?: (sel: TSel) => void;
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

  /**
   * 单个节点的展示视图。
   *
   * 判定全部来自 `evaluateBrowseNode`（wire 契约），本函数只补「语义载荷能否
   * 交给上层」这一层：处理器缺失 / 上层未接线 / 处理器拒绝，三者都转成**可见
   * 阻断**，绝不静默丢弃。
   */
  const viewOf = (node: BrowseNode): LeafView => {
    const evaluation = evaluateBrowseNode(node);
    const blocks: BrowseBlock[] = [...evaluation.blocks];
    let semantic: LeafView["semantic"] = null;

    if (evaluation.semanticPayload) {
      if (!semanticHandler) {
        blocks.push(missingHandlerBlock(DEFAULT_MISSING_HANDLER_REASON));
      } else if (!onSemanticSelect) {
        blocks.push(
          missingHandlerBlock(semanticHandler.notProvidedReason ?? DEFAULT_MISSING_HANDLER_REASON),
        );
      } else {
        const parsed = semanticHandler.parse(evaluation.semanticPayload);
        if (parsed.ok) {
          semantic = {
            selection: parsed.selection,
            label: parsed.label ?? semanticHandler.selectLabel ?? "选用",
          };
        } else {
          blocks.push(parseRejectedBlock(parsed.code, parsed.message));
        }
      }
    }

    return { resource: evaluation.resource, notes: evaluation.notes, blocks, semantic };
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
            const view = viewOf(n);
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
                  {view.notes.map((note) => (
                    <span
                      key={`${note.code}-${note.message}`}
                      data-testid={`browse-note-${note.code || "anonymous"}`}
                      style={{
                        display: "block",
                        fontSize: 11,
                        color: note.tone === "warn" ? "#8c6000" : "#525252",
                      }}
                    >
                      {note.message}
                    </span>
                  ))}
                  {view.blocks.map((b) => (
                    <span
                      key={`${b.code}-${b.message}`}
                      data-testid={b.testId}
                      style={{ display: "block", fontSize: 11, color: "#c62828" }}
                    >
                      不可选用（{b.code}）：{b.message}
                    </span>
                  ))}
                </span>
                {n.kind ? <Tag>{n.kind}</Tag> : null}
                {n.data_type ? <Tag>{n.data_type}</Tag> : null}
                {n.access && n.access !== "read" ? <Tag color="orange">{n.access}</Tag> : null}
                <Space>
                  {n.has_children ? (
                    <Button size="small" onClick={() => enter(n.id)}>进入</Button>
                  ) : null}
                  {view.resource ? (
                    <Button size="small" type="primary" onClick={() => onFill(view.resource as BrowseSelection)}>
                      选用
                    </Button>
                  ) : null}
                  {view.semantic ? (
                    <Button
                      size="small"
                      type="primary"
                      onClick={() => onSemanticSelect?.(view.semantic?.selection as TSel)}
                    >
                      {view.semantic.label}
                    </Button>
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
