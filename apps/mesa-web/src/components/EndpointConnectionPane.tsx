// Endpoint 连接编辑窗格（Workspace「连接」tab；逻辑由 DeviceDetail 编辑
// Modal 原样迁移）。driver_id 创建后不可改（请求体无该字段）；保存先停止，
// 成功后需手动启动（生命周期统一见后续项，本窗格保持原语义）。
import { useEffect, useState } from "react";
import { Button, Input, Space, Tag, message } from "antd";
import { api } from "../api";
import type { DriverDescriptor } from "../types";
import { buildEndpointUpdatePayload, cleanConnection } from "../deviceModel";
import { DescriptorFields } from "./DescriptorFields";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export function EndpointConnectionPane({
  endpointId,
  deviceId,
  driverId,
  initialName,
  onChanged,
}: {
  endpointId: string;
  deviceId: string;
  driverId: string;
  initialName: string;
  onChanged: () => void;
}) {
  const [desc, setDesc] = useState<DriverDescriptor | null>(null);
  const [conn, setConn] = useState<Record<string, unknown>>({});
  const [name, setName] = useState(initialName);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    fetch(`/api/v1/endpoints/${endpointId}`)
      .then((x) => x.json())
      .then((r) => {
        // 服务端可能脱敏 Secret 字段；未动字段原样回传由后端合并语义决定
        setConn((r.connection as Record<string, unknown>) ?? {});
        setName(r.name ?? r.id ?? endpointId);
      })
      .catch(() => message.error("获取连接失败"));
    fetch(`/api/v1/drivers/${driverId}/descriptor`)
      .then((x) => x.json())
      .then((d) => setDesc(d))
      .catch(() => setDesc(null));
  }, [endpointId, driverId]);

  const save = async () => {
    const cleaned = cleanConnection(conn);
    if (!Object.keys(cleaned).length) return message.warning("请填写连接参数");
    setSaving(true);
    try {
      await api.stopEndpoint(endpointId).catch(() => {});
      await sleep(300);
      // driver_id 不可变：请求体无该字段（误带即后端 deny_unknown_fields 拒绝）
      const body = buildEndpointUpdatePayload({ name: name.trim() || endpointId, deviceId, connection: cleaned });
      await api.updateEndpoint(endpointId, body);
      message.success("修改成功（驱动未变，需手动启动）");
      onChanged();
    } catch (e) {
      const err = e as { message?: string };
      message.error(err?.message ?? "修改失败");
    } finally {
      setSaving(false);
    }
  };

  return (
    <div style={{ display: "grid", gap: 12, maxWidth: 720 }}>
      <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
        <span style={{ fontSize: 12 }}>驱动</span>
        <Tag>{driverId}</Tag>
        <span style={{ fontSize: 12, color: "#525252" }}>创建后不可改</span>
      </div>
      <div>
        <div style={{ fontSize: 12, marginBottom: 4 }}>连接名称</div>
        <Input value={name} onChange={(e) => setName(e.target.value)} />
      </div>
      {!desc ? (
        <div style={{ color: "#525252" }}>加载中…</div>
      ) : (
        <DescriptorFields schema={desc.connection} value={conn} onChange={setConn} />
      )}
      <div style={{ fontSize: 12, color: "#525252" }}>需先停止再修改（已自动停止），保存后需手动启动</div>
      <Space>
        <Button type="primary" onClick={save} loading={saving}>保存</Button>
      </Space>
    </div>
  );
}
