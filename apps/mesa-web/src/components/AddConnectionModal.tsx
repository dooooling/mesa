// 新增连接 Modal（C2）：驱动选择 / 连接参数 / 校验 / 探测 / 创建全部交给共享面板。
//
// 面板负责（三入口同源）：
// - 驱动清单只认服务端 `launchable=true`，拉取失败即报错 + 重试并禁止提交
//   （**已删除**原 `FALLBACK_DRIVERS` 常量：不再把内置列表当可用驱动展示）；
// - 描述符失败即禁止提交；切换驱动按新 Schema 重建参数，不残留旧驱动参数；
// - TSAP 未失焦提交最新输入；非法输入阻断且不自动修复；
// - 参数一变旧的校验/探测结果立即失效。
// 本文件只负责「名称/ID」这两个非连接字段与最终 POST /endpoints。
import { useState } from "react";
import { Button, Form, Input, Modal, message } from "antd";
import { api } from "../api";
import { buildEndpointCreatePayload, suggestEndpointId } from "../deviceModel";
import { safeText, type ConnectionValues } from "../connection/connectionSetupModel";
import { ConnectionSetupPanel } from "../connection/ConnectionSetupPanel";

export function AddConnectionModal(props: {
  deviceId: string;
  open: boolean;
  onClose: () => void;
  onCreated: () => void;
}) {
  const { deviceId, open, onClose, onCreated } = props;
  const [form] = Form.useForm();
  const [driverId, setDriverId] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const handleSubmit = async (args: {
    driverId: string;
    connection: ConnectionValues;
  }) => {
    let values: { name?: string; id?: string };
    try {
      values = (await form.validateFields()) as { name?: string; id?: string };
    } catch {
      // antd 表单校验未通过（连接名称必填）：不落库、不提示成功
      return;
    }
    setSubmitting(true);
    try {
      const payload = buildEndpointCreatePayload({
        deviceId,
        driverId: args.driverId,
        name: (values.name ?? "").trim() || "connection",
        connection: args.connection,
        id: (values.id ?? "").trim() || undefined,
      });
      const r = await api.createEndpoint(payload);
      if (r.status !== 201 && r.status !== 200) {
        message.error(safeText((r.body as { error?: unknown })?.error) || "创建连接失败");
        return;
      }
      message.success(`已添加连接 ${payload.id}（归属 ${deviceId}，未新增设备）`);
      onCreated();
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Modal
      title={`新增连接 · 归属 ${deviceId}`}
      open={open}
      onCancel={onClose}
      footer={null}
      destroyOnHidden
      width={720}
    >
      <ConnectionSetupPanel
        mode="add"
        active={open}
        sessionKey={`add-connection:${deviceId}`}
        submitLabel="创建"
        actionsDisabled={submitting}
        onSubmit={handleSubmit}
        onDriverChange={setDriverId}
        header={
          <Form form={form} layout="vertical">
            <Form.Item
              name="name"
              label="连接名称"
              rules={[{ required: true, message: "连接名称必填" }]}
            >
              <Input placeholder="如 PLC / NCK / OPC UA" />
            </Form.Item>
            <Form.Item name="id" label="连接 ID（可空自动生成，与设备 ID 独立）">
              <Input
                placeholder={suggestEndpointId(driverId ?? "connection")}
                style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace" }}
              />
            </Form.Item>
          </Form>
        }
      />
      <div style={{ marginTop: 12 }}>
        <Button onClick={onClose}>取消</Button>
      </div>
    </Modal>
  );
}
