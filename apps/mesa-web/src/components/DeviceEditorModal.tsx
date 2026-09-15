// 设备编辑 Modal（逻辑由 DeviceConfig 原样迁移）：
// 改名（PUT device）+ Danger Zone 删除设备（canDeleteDevice 预检，
// 后端 RESTRICT 为最终裁决；删后回 /devices）。
import { useState } from "react";
import { Button, Divider, Form, Input, Modal, message } from "antd";
import { useNavigate } from "react-router-dom";
import { api } from "../api";
import { canDeleteDevice } from "../deviceModel";
import type { WorkspaceEndpoint } from "../workspace/useDeviceWorkspaceData";

export function DeviceEditorModal(props: {
  deviceId: string;
  deviceName: string;
  endpoints: WorkspaceEndpoint[];
  open: boolean;
  onClose: () => void;
  onChanged: () => void;
}) {
  const { deviceId, deviceName, endpoints, open, onClose, onChanged } = props;
  const nav = useNavigate();
  const [form] = Form.useForm();
  const [saving, setSaving] = useState(false);

  const saveRename = async () => {
    try {
      const v = (await form.validateFields()) as { name: string };
      setSaving(true);
      await api.updateDevice(deviceId, { name: v.name.trim() });
      message.success("设备已改名");
      onChanged();
    } catch (e) {
      const err = e as { status?: number; message?: string };
      if (err?.status) message.error(err?.message ?? "改名失败");
    } finally {
      setSaving(false);
    }
  };

  const removeDevice = () => {
    const pre = canDeleteDevice(deviceId, endpoints);
    if (!pre.ok) {
      message.warning(pre.reason);
      return;
    }
    Modal.confirm({
      title: `删除设备 ${deviceId}？`,
      content: "设备删除后不可恢复；其连接必须已清空（仍有连接时后端会拒绝）。",
      okText: "删除",
      okType: "danger",
      cancelText: "取消",
      onOk: async () => {
        const r = await api.deleteDevice(deviceId);
        if (r.status !== 200) {
          message.error(r.body?.error?.message ?? "删除失败");
          return;
        }
        message.success("已删除设备");
        nav("/devices");
      },
    });
  };

  return (
    <Modal
      title={`编辑设备 · ${deviceId}`}
      open={open}
      onCancel={onClose}
      footer={null}
      destroyOnHidden
    >
      <Form
        form={form}
        layout="vertical"
        initialValues={{ name: deviceName }}
        onFinish={saveRename}
      >
        <Form.Item label="ID（只读）">
          <Input value={deviceId} disabled style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace" }} />
        </Form.Item>
        <Form.Item name="name" label="设备名称" rules={[{ required: true, message: "设备名称必填" }]}>
          <Input />
        </Form.Item>
        <Form.Item>
          <Button type="primary" htmlType="submit" loading={saving}>
            保存
          </Button>
        </Form.Item>
      </Form>
      <Divider />
      <div style={{ fontSize: 13, fontWeight: 600, color: "#da1e28", marginBottom: 8 }}>Danger Zone</div>
      <Button danger onClick={removeDevice}>
        删除设备
      </Button>
    </Modal>
  );
}
