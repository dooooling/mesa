// PR27 Device-first：Device 列表页。只展示 Device（GET /devices），
// 数量只数 Device；Endpoint 数量另算（/endpoints），仅作为每行的
// “连接数”附属信息展示，绝不混入 Device 计数。行点击进入 Device detail。
import { useEffect, useState } from "react";
import { Button, Card, Form, Input, Modal, Space, Table, Tag, message } from "antd";
import { useNavigate } from "react-router-dom";
import { api } from "../api";
import { deviceCounts, groupEndpointsByDevice, isRunningState, type Device } from "../deviceModel";

// M1 设备状态徽标：仅按连接运行态聚合展示（至少一个 RUNNING/CONNECTING/
// RECONNECTING 即“正常”，无连接即“停止”，其余为“异常”）。不是健康模型，
// M2 数据健康（GOOD/BAD/STALE）接入后与此列组合。
function DeviceStatusBadge({ states }: { states: string[] }) {
  if (states.length === 0) return <Tag>○ 停止</Tag>;
  if (states.some((s) => isRunningState(s))) return <Tag color="green">● 正常</Tag>;
  return <Tag color="red">● 异常</Tag>;
}

export function DevicesPage() {
  const nav = useNavigate();
  const [devices, setDevices] = useState<Device[]>([]);
  const [epCountByDevice, setEpCountByDevice] = useState<Map<string, number>>(new Map());
  const [epStateByDevice, setEpStateByDevice] = useState<Map<string, string[]>>(new Map());
  const [endpointTotal, setEndpointTotal] = useState(0);
  const [open, setOpen] = useState(false);
  // M1 瘦身：搜索框（按 ID/名称过滤），整行点击进入 Workspace。
  const [search, setSearch] = useState("");
  const [form] = Form.useForm();

  const load = async () => {
    try {
      const dj = (await api.listDevices()) as { devices?: Device[] };
      const ds = dj.devices ?? [];
      setDevices(ds);
      const ej = (await api.listEndpoints()) as { endpoints?: Array<{ id: string; device_id?: string; state?: string; runtime?: { state?: string } }> };
      const eps = ej.endpoints ?? [];
      const groups = groupEndpointsByDevice(
        eps.map((e) => ({ id: e.id, driver_id: "", device_id: e.device_id })),
      );
      const m = new Map<string, number>();
      for (const [k, v] of groups) if (k) m.set(k, v.length);
      setEpCountByDevice(m);
      // M1 状态列：按设备聚合连接运行态（展示用，不做健康模型判定）。
      const states = new Map<string, string[]>();
      for (const e of eps) {
        if (!e.device_id) continue;
        const list = states.get(e.device_id) ?? [];
        list.push(e.state ?? e.runtime?.state ?? "");
        states.set(e.device_id, list);
      }
      setEpStateByDevice(states);
      setEndpointTotal(eps.length);
    } catch {
      message.error("加载设备列表失败");
    }
  };
  useEffect(() => { load(); }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const { deviceCount } = deviceCounts(devices, []);

  // M1：搜索按 ID/名称子串过滤；行点击进入 Device Workspace 概览。
  const filtered = devices.filter((d) => {
    const q = search.trim().toLowerCase();
    if (!q) return true;
    return d.id.toLowerCase().includes(q) || (d.name ?? "").toLowerCase().includes(q);
  });

  const gotoWorkspace = (id: string) => nav(`/devices/${id}/overview`);

  const create = async () => {
    try {
      const v = (await form.validateFields()) as { id: string; name?: string };
      const id = v.id.trim();
      // 名称可空（placeholder“默认为 ID”）：未填时用 id；可选链避免
      // undefined.trim() 抛异常后被空 catch 当校验失败吞掉的静默失败。
      const name = v.name?.trim() || id;
      const r = await api.createDevice({ id, name });
      if (r.status !== 201 && r.status !== 200) {
        message.error(r.body?.error?.message ?? "创建设备失败");
        return;
      }
      message.success(`已创建设备 ${id}`);
      setOpen(false);
      form.resetFields();
      load();
    } catch { /* antd 校验未通过 */ }
  };

  const remove = async (id: string) => {
    const n = epCountByDevice.get(id) ?? 0;
    if (n > 0) {
      message.warning(`该设备下仍有 ${n} 个连接，请先到设备详情删除关联 Endpoint`);
      return;
    }
    // 破坏性操作明确确认：说明后果，不可恢复
    Modal.confirm({
      title: `删除设备 ${id}？`,
      content: "设备删除后不可恢复。仍有连接时后端会拒绝，请先清空连接。",
      okText: "删除",
      okType: "danger",
      cancelText: "取消",
      onOk: async () => {
        const r = await api.deleteDevice(id);
        if (r.status !== 200) {
          // 后端 RESTRICT 为最终裁决（如竞态下仍有 Endpoint），直接透传服务端文案
          message.error(r.body?.error?.message ?? "删除失败");
          load();
          return;
        }
        message.success("已删除设备");
        load();
      },
    });
  };

  return (
    <div style={{ display: "grid", gap: 16 }}>
      <Card
        size="small"
        title={`设备 · ${deviceCount}`}
        extra={
          <Space>
            <Button onClick={() => nav("/onboarding")}>新建向导</Button>
            <Button type="primary" onClick={() => setOpen(true)}>+ 添加设备</Button>
          </Space>
        }
      >
        <Input
          placeholder="搜索设备（ID / 名称）"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          allowClear
          style={{ maxWidth: 320, marginBottom: 12 }}
        />
        <Table
          size="small"
          rowKey="id"
          dataSource={filtered}
          onRow={(r) => ({ onClick: () => gotoWorkspace(r.id), style: { cursor: "pointer" } })}
          columns={[
            { title: "名称", dataIndex: "name", render: (v: string) => v ?? "—" },
            { title: "状态", dataIndex: "id", render: (id: string) => <DeviceStatusBadge states={epStateByDevice.get(id) ?? []} /> },
            {
              title: "连接",
              dataIndex: "id",
              render: (id: string) => {
                const n = epCountByDevice.get(id) ?? 0;
                return n > 0 ? <Tag color="blue">{n} 个连接</Tag> : <Tag>无连接</Tag>;
              },
            },
            {
              // M1 只给列占位：数据健康（GOOD/BAD/STALE 汇总）M2 接入点位数据后填充。
              title: "数据状态",
              render: () => <span style={{ color: "#8c8c8c" }}>M2 接入</span>,
            },
            {
              // M1 只给列占位：最近更新 M2 接入点位时间戳后填充。
              title: "最近更新",
              render: () => <span style={{ color: "#8c8c8c" }}>—</span>,
            },
            {
              title: "操作",
              render: (_: unknown, r: Device) => (
                <Space onClick={(e) => e.stopPropagation()}>
                  {/* M1 已删除“进入”按钮：整行点击即进入 Workspace，不再重复操作。
                      删除保留在列表 ⋯ 的最小形态，M4 改名/删除收敛到配置页后移除。 */}
                  <Button size="small" danger onClick={() => remove(r.id)}>删除</Button>
                </Space>
              ),
            },
          ]}
          locale={{ emptyText: "暂无设备，先新建设备或走新建向导" }}
        />
        <div style={{ marginTop: 8, fontSize: 12, color: "#525252" }}>
          设备 {deviceCount} 个 · 连接 {endpointTotal} 个（连接归属设备，不计入设备数）
        </div>
      </Card>

      <Modal title="新建设备" open={open} onOk={create} onCancel={() => setOpen(false)} okText="创建" destroyOnHidden>
        <Form form={form} layout="vertical">
          <Form.Item name="id" label="设备 ID" rules={[{ required: true, message: "设备 ID 必填" }]}>
            <Input placeholder="device-a" style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace" }} />
          </Form.Item>
          <Form.Item name="name" label="设备名称">
            <Input placeholder="默认为 ID" />
          </Form.Item>
          <div style={{ fontSize: 12, color: "#525252" }}>只创建 Device；连接在设备详情页按需添加，可一对多。</div>
        </Form>
      </Modal>
    </div>
  );
}
