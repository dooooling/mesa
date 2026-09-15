// 连接一级页：只管理 Endpoint（增删改查/启停），禁止放
// Acquisition / Event Subscription / Diagnostics。
import { useState } from "react";
import { Button, Space, Table, Tag, Modal, message } from "antd";
import { api } from "../api";
import { isRunningState } from "../deviceModel";
import type { WorkspaceEndpoint } from "./useDeviceWorkspaceData";
import { AddConnectionModal } from "../components/AddConnectionModal";
import { ConnectionEditorModal } from "../components/ConnectionEditorModal";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export function DeviceConnections(props: {
  deviceId: string;
  endpoints: WorkspaceEndpoint[];
  /** 变更后刷新 inventory（Header 连接数/状态实时跟上）。 */
  onReload: () => void;
}) {
  const { deviceId, endpoints, onReload } = props;
  const [addOpen, setAddOpen] = useState(false);
  const [editId, setEditId] = useState<string | null>(null);
  const editTarget = endpoints.find((e) => e.id === editId) ?? null;

  const act = async (id: string, a: "start" | "stop" | "delete") => {
    if (a === "delete") {
      await api.stopEndpoint(id).catch(() => {});
      await sleep(300);
      const r = await api.deleteEndpoint(id);
      if (r.status !== 200) {
        message.error(r.body?.error?.message ?? "删除失败");
        onReload();
        return;
      }
      message.success("已删除连接，所属设备保留");
      onReload();
      return;
    }
    const r = a === "stop" ? await api.stopEndpoint(id) : await api.startEndpoint(id);
    if (r.status !== 200) {
      message.error(r.body?.error?.message ?? (a === "stop" ? "停止失败" : "启动失败"));
      onReload();
      return;
    }
    message.success(a === "stop" ? "已停止" : "已启动");
    onReload();
  };

  const confirmDelete = (id: string, name?: string) => {
    Modal.confirm({
      title: `删除连接 ${name ?? id}？`,
      content: "连接删除后其采集/事件配置一并清除，不可恢复；所属设备保留。",
      okText: "删除",
      okType: "danger",
      cancelText: "取消",
      onOk: () => act(id, "delete"),
    });
  };

  return (
    <div style={{ display: "grid", gap: 16 }}>
      <div style={{ display: "flex", alignItems: "center" }}>
        <h3 style={{ fontSize: 14, fontWeight: 600, margin: 0 }}>连接</h3>
        <Space style={{ marginLeft: "auto" }}>
          <Button size="small" type="primary" onClick={() => setAddOpen(true)}>
            + 新增连接
          </Button>
        </Space>
      </div>
      <Table
        size="small"
        rowKey="id"
        dataSource={endpoints}
        columns={[
          { title: "名称", dataIndex: "name", render: (v: string) => v ?? "—" },
          { title: "ID", dataIndex: "id", render: (v: string) => <span style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 12 }}>{v}</span> },
          { title: "驱动", dataIndex: "driver_id", render: (v: string) => <Tag>{v}</Tag> },
          {
            title: "状态",
            dataIndex: "state",
            render: (v: string) => <Tag color={isRunningState(v) ? "green" : (v ?? "").toUpperCase() === "FAILED" ? "red" : "default"}>{v ?? "—"}</Tag>,
          },
          {
            title: "操作",
            render: (_: unknown, r: WorkspaceEndpoint) => {
              const running = isRunningState(r.state);
              return (
                <Space onClick={(e) => e.stopPropagation()}>
                  <Button size="small" onClick={() => setEditId(r.id)}>编辑</Button>
                  {!running
                    ? <Button size="small" type="primary" onClick={() => act(r.id, "start")}>启动</Button>
                    : <Button size="small" onClick={() => act(r.id, "stop")}>停止</Button>}
                  <Button size="small" danger onClick={() => confirmDelete(r.id, r.name)}>删除</Button>
                </Space>
              );
            },
          },
        ]}
        locale={{ emptyText: "该设备下暂无连接，点“新增连接”添加第一个连接（不会新增设备）" }}
      />
      <AddConnectionModal
        deviceId={deviceId}
        open={addOpen}
        onClose={() => setAddOpen(false)}
        onCreated={() => { setAddOpen(false); onReload(); }}
      />
      {editTarget ? (
        <ConnectionEditorModal
          key={editTarget.id}
          endpointId={editTarget.id}
          deviceId={deviceId}
          driverId={editTarget.driver_id}
          initialName={editTarget.name ?? editTarget.id}
          open={editId !== null}
          onClose={() => setEditId(null)}
          onChanged={() => { setEditId(null); onReload(); }}
        />
      ) : null}
    </div>
  );
}
