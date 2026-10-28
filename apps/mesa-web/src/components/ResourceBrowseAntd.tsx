// Descriptor 驱动的 Browse 选型（P0-2）：消费 resource_selection_methods 声明的
// Browse 能力。
//
// 本组件**只**理解 browse 节点信封（见 browseModel.ts）：下钻靠 has_children +
// 节点 id 回传 parent；回根即 parent=""。它不认识任何厂商/协议/Point Kind：
//
// - 通用资源叶：`selectableFromNode` → `onFill`（resource + parameters 回填）；
// - 语义叶：由调用方注入的 `BrowseLeafPolicy` 判定（是否可选、类别标签、
//   不可选原因、预览提示、选用载荷），本组件只负责展示与转发 → `onLeafSelect`；
// - 载荷声明了 point kind 但没有策略接管：fail-closed，不给「选用」入口。
//
// 身份纪律（与驱动无关的通用要求）：节点自带的预览身份**只展示**，永不作为
// 最终身份；提交前必须由服务端 Preview / Configure 重新确认（策略层不得放宽）。
import { useEffect, useState } from "react";
import { Button, Input, Space, Tag, message } from "antd";
import {
  buildBrowseRequest,
  declaresOpaquePointKind,
  parseBrowseEnvelope,
  selectableFromNode,
  type BrowseNode,
  type BrowseSelection,
} from "../browseModel";
import {
  DEFAULT_MISSING_HANDLER_REASON,
  NO_POLICY_FOR_POINT_KIND_REASON,
  type BrowseLeafOwned,
  type BrowseLeafPolicy,
} from "../browseLeafPolicy";

export function ResourceBrowseAntd<TSel>({
  endpointId,
  onFill,
  leafPolicy,
  onLeafSelect,
}: {
  endpointId: string;
  /** 用户选用通用资源叶：父层切到手动表单并预填（resource + parameters）。 */
  onFill: (sel: BrowseSelection) => void;
  /**
   * 叶判定策略（可选）。由驱动语义层注入；未注入时所有带语义载荷的叶
   * 一律不可选用（fail-closed），通用资源叶行为逐字不变。
   */
  leafPolicy?: BrowseLeafPolicy<TSel> | null;
  /**
   * 用户选用策略接管的叶：父层按载荷（不透明）走自己的表单。
   * 未提供时策略叶的「选用」入口关闭（不回落到通用资源回填）。
   */
  onLeafSelect?: (sel: TSel) => void;
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
   * 优先级：策略接管 > 通用资源叶 > 「载荷需语义层解释但未注入策略」的阻断。
   * 三者互斥，且任何一条不成立时都不产生「选用」入口（fail-closed）。
   */
  const viewOf = (node: BrowseNode) => {
    const verdict = leafPolicy?.evaluate(node) ?? { mode: "generic" as const };
    if (verdict.mode === "owned") {
      let owned: BrowseLeafOwned<TSel> = verdict;
      if (owned.selectable) {
        // 策略说可选，但缺少载荷或上层未接线 ⇒ 退化为不可选并显示原因，
        // 绝不把「策略的乐观结论」当成可提交。
        if (!owned.selection) {
          owned = { ...owned, selectable: false, reason: "策略未提供可选用载荷：不可选用" };
        } else if (!onLeafSelect) {
          owned = {
            ...owned,
            selectable: false,
            reason: leafPolicy?.missingHandlerReason ?? DEFAULT_MISSING_HANDLER_REASON,
          };
        }
      }
      return { owned, generic: null as BrowseSelection | null, unclaimedKindPayload: false };
    }
    const generic = selectableFromNode(node);
    return {
      owned: null as BrowseLeafOwned<TSel> | null,
      generic,
      unclaimedKindPayload: !generic && declaresOpaquePointKind(parseBrowseEnvelope(node)),
    };
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
            const { owned, generic } = view;
            const selectable = !!owned?.selectable && !!owned.selection && !!onLeafSelect;
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
                  {owned?.badge || (owned?.tags?.length ?? 0) > 0 ? (
                    <span style={{ display: "block", fontSize: 11, color: "#525252" }}>
                      {owned?.badge ? <Tag color="blue">{owned.badge}</Tag> : null}
                      {owned?.tags?.map((tag) => <Tag key={tag}>{tag}</Tag>)}
                      {owned?.preview ? (
                        <span
                          style={{
                            fontFamily: "'IBM Plex Mono',monospace",
                            color: owned.preview.tone === "warn" ? "#8c6000" : undefined,
                          }}
                        >
                          {owned.preview.text}
                        </span>
                      ) : null}
                    </span>
                  ) : null}
                  {owned?.notice ? (
                    <span
                      data-testid={owned.notice.testId}
                      style={{ display: "block", fontSize: 11, color: "#c62828" }}
                    >
                      {owned.notice.text}
                    </span>
                  ) : null}
                  {owned && !owned.selectable && owned.reason ? (
                    <span
                      data-testid={owned.blockTestId ?? "browse-leaf-block"}
                      style={{ display: "block", fontSize: 11, color: "#c62828" }}
                    >
                      不可选用：{owned.reason}
                    </span>
                  ) : null}
                  {view.unclaimedKindPayload ? (
                    <span
                      data-testid="browse-leaf-block"
                      style={{ display: "block", fontSize: 11, color: "#c62828" }}
                    >
                      不可选用：{NO_POLICY_FOR_POINT_KIND_REASON}
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
                  {owned?.selectable ? (
                    selectable ? (
                      <Button
                        size="small"
                        type="primary"
                        onClick={() => onLeafSelect?.(owned.selection as TSel)}
                      >
                        {owned.selectLabel ?? "选用"}
                      </Button>
                    ) : null
                  ) : generic ? (
                    <Button size="small" type="primary" onClick={() => onFill(generic)}>选用</Button>
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
