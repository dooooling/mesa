// 采集一级页：定义什么数据进入 Mesa（周期数据 + 事件数据）。
// 两个 Section（数据采集 / 事件订阅），顶部 ConnectionSelect（单选）。
//
// 第 2 步（页面交互）在本层补两件外壳职责（§4.7 / §7-7）：
// 1. **切换连接前**提示未保存修改；保存中禁止切换；
// 2. 把「有未保存修改」注册为浏览器原生离开提醒，并上报给设备页做 Tab 切换守卫。
// 采集窗格本身只报告状态（`onDraftStateChange`），不自己拦路由。
import { Alert, Card } from "antd";
import { useCallback, useMemo, useState } from "react";
import { EndpointAcquisitionPane } from "../components/EndpointAcquisitionPane";
import { EventTaskEditor } from "../components/EventTaskEditor";
import { confirmDiscardDraft, useBeforeUnloadGuard } from "../acquisitionDraftGuard";
import { ConnectionSelect } from "./ConnectionSelect";
import { useConnectionQuery } from "./useConnectionQuery";
import type { WorkspaceEndpoint } from "./useDeviceWorkspaceData";

export function DeviceAcquisition(props: {
  endpoints: WorkspaceEndpoint[];
  endpointsReady: boolean;
  /** 变更后刷新 inventory。 */
  onReload: () => void;
  /** 上报「有未保存的采集配置修改」（设备页用于 Tab 切换提醒）。 */
  onDraftDirtyChange?: (dirty: boolean) => void;
}) {
  const { endpoints, endpointsReady, onReload, onDraftDirtyChange } = props;
  const endpointIds = useMemo(() => endpoints.map((e) => e.id), [endpoints]);
  const { selected, select } = useConnectionQuery({ endpointIds, mode: "single", ready: endpointsReady });
  const active = endpoints.find((e) => e.id === selected) ?? null;
  // 草稿状态（dirty = 有未保存修改，saving = 正在保存）：切换连接与离开页面的唯一依据。
  const [draft, setDraft] = useState<{ dirty: boolean; saving: boolean }>({
    dirty: false,
    saving: false,
  });
  const handleDraftState = useCallback(
    (state: { dirty: boolean; saving: boolean }) => {
      setDraft(state);
      onDraftDirtyChange?.(state.dirty);
    },
    [onDraftDirtyChange],
  );
  // 刷新/关闭标签页：浏览器原生离开提醒（不静默丢弃）。
  useBeforeUnloadGuard(draft.dirty);

  if (!endpoints.length) {
    return <Alert type="warning" showIcon message="该设备暂无连接" description="请先到「连接」页新增连接后再配置采集。" />;
  }

  return (
    <div style={{ display: "grid", gap: 16 }}>
      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
        <h3 style={{ fontSize: 14, fontWeight: 600, margin: 0 }}>采集</h3>
        <span style={{ marginLeft: "auto" }}>
          <ConnectionSelect
            endpoints={endpoints}
            value={selected}
            allowAll={false}
            disabled={draft.saving}
            onChange={(next) => {
              // 保存中禁止切换连接（§4.7）：请求还在飞时换页面会错位。
              if (draft.saving) return;
              void (async () => {
                if (!(await confirmDiscardDraft(draft.dirty, "切换连接"))) return;
                select(next);
              })();
            }}
          />
        </span>
      </div>
      {active ? (
        <div key={active.id} style={{ display: "grid", gap: 16 }}>
          <Card size="small" title={`数据采集 · ${active.name ?? active.id}`}>
            <EndpointAcquisitionPane
              endpointId={active.id}
              driverId={active.driver_id}
              onChanged={onReload}
              onDraftStateChange={handleDraftState}
            />
          </Card>
          <Card size="small" title="事件订阅">
            <EventTaskEditor fixedEndpointId={active.id} />
          </Card>
        </div>
      ) : null}
    </div>
  );
}
