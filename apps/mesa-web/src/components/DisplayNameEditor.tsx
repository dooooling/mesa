// P2 展示名编辑器：改名只写 registry + 同步 snapshot，不碰
// point_id/key/label，Driver 无感知。空输入 = 清除（回落 point_key）。
import { useEffect, useState } from "react";
import { Button, Input, Space, message } from "antd";
import type { DevicePointView } from "../workspace/useDeviceWorkspaceData";

async function putDisplayName(endpoint_id: string, point_key: string, display_name: string | null) {
  const r = await fetch(
    `/api/v1/endpoints/${encodeURIComponent(endpoint_id)}/points/${encodeURIComponent(point_key)}/display-name`,
    {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ display_name }),
    },
  );
  if (!r.ok) {
    let detail = "";
    try {
      detail = ((await r.json()) as { error?: { message?: string } }).error?.message ?? "";
    } catch {
      /* 忽略解析失败 */
    }
    throw new Error(detail || `HTTP ${r.status}`);
  }
}

/** 改名目标身份（RC2 scope 生命周期口径）：PUT 返回时父组件用它
 * 定位回填目标，而不是“此刻 Drawer 里开的是谁”。 */
export interface RenameTarget {
  endpoint_id: string;
  point_key: string;
}

export function DisplayNameEditor(props: {
  point: DevicePointView;
  /**
   * 改名成功回调：永远 patch 回请求时的 target；
   * 父组件只有当前 openPoint identity == target 才刷新 Drawer。
   */
  onRenamed: (target: RenameTarget, display_name: string | null) => void;
}) {
  const { point, onRenamed } = props;
  const [draft, setDraft] = useState(point.display_name ?? "");
  const [saving, setSaving] = useState(false);
  // 切点时重置输入框（Drawer 复用同一组件实例）。
  useEffect(() => {
    setDraft(point.display_name ?? "");
  }, [point.endpoint_id, point.point_key, point.display_name]);

  const save = async (name: string | null) => {
    // 发请求前捕获身份：pending 期间切点/关 Drawer，返回仍回填 A。
    const target: RenameTarget = {
      endpoint_id: point.endpoint_id,
      point_key: point.key ?? point.point_key ?? "",
    };
    setSaving(true);
    try {
      await putDisplayName(target.endpoint_id, target.point_key, name);
      onRenamed(target, name);
      message.success(name === null ? "已清除展示名" : "展示名已保存");
    } catch (e) {
      message.error(`保存失败：${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setSaving(false);
    }
  };

  const trimmed = draft.trim();
  return (
    <Space.Compact style={{ width: "100%" }}>
      <Input
        value={draft}
        maxLength={128}
        placeholder="未设置（显示 point_key）"
        onChange={(e) => setDraft(e.target.value)}
        onPressEnter={() => { if (trimmed) void save(trimmed); }}
        disabled={saving}
      />
      <Button type="primary" disabled={saving || !trimmed} onClick={() => void save(trimmed)}>
        保存
      </Button>
      <Button disabled={saving || !point.display_name} onClick={() => void save(null)}>
        清除
      </Button>
    </Space.Compact>
  );
}
