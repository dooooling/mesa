// Descriptor-driven Browse 选型（P0-2）：消费 resource_selection_methods
// 声明的 Browse 能力。只理解 browse 节点信封（见 browseModel），不理解
// 任何协议：下钻靠 has_children + 节点 id 回传 parent；回根即 parent=""；
// 选用须经 selectableFromNode 判定，无 binding 的分支只能进入。
import { useEffect, useState } from "react";
import { Button, Input, Space, Tag, message } from "antd";
import { buildBrowseRequest, selectableFromNode, type BrowseNode, type BrowseSelection } from "../browseModel";

export function ResourceBrowseAntd({
  endpointId,
  onFill,
}: {
  endpointId: string;
  /** 用户选用某节点：父层切到手动表单并预填（resource + parameters），输出仍由用户勾选 */
  onFill: (sel: BrowseSelection) => void;
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
                </span>
                {n.kind ? <Tag>{n.kind}</Tag> : null}
                {n.data_type ? <Tag>{n.data_type}</Tag> : null}
                {n.access && n.access !== "read" ? <Tag color="orange">{n.access}</Tag> : null}
                <Space>
                  {n.has_children ? (
                    <Button size="small" onClick={() => enter(n.id)}>进入</Button>
                  ) : null}
                  {sel ? (
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
