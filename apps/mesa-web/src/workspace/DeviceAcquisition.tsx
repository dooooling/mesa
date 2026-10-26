// 采集一级页：定义什么数据进入 Mesa（周期数据 + 事件数据）。
// 两个 Section（数据采集 / 事件订阅），顶部 ConnectionSelect（单选）。
//
// 第 2 步（页面交互）在本层补两件外壳职责（§4.7 / §7-7）：
// 1. **切换连接前**提示未保存修改；保存中禁止切换；
// 2. 把「有未保存修改」注册为浏览器原生离开提醒，并上报给设备页做 Tab 切换守卫。
// 采集窗格本身只报告状态（`onDraftStateChange`），不自己拦路由。
//
// 第 4 步修复（§2.5 第 8 条 + §7-8 定案）：事件订阅区域**按驱动声明的事件能力**出现——
// 之前无条件渲染，不支持事件流的驱动也会常驻一块「该驱动未声明事件流」的卡片。
// 判据是 `descriptor.events.streams` 是否为空（与 `EventTaskEditor.tsx:129`、`:330-331`
// 同源），**不看 `driver_id`**；不支持 ⇒ 整块不渲染；支持 ⇒ 独立折叠区域（默认收起），
// 保存范围仍是这一块自己的（`EventTaskEditor` 内部的生命周期）。
import { Alert, Card, Collapse } from "antd";
import { useCallback, useMemo, useState } from "react";
import { EndpointAcquisitionPane } from "../components/EndpointAcquisitionPane";
import { EventTaskEditor } from "../components/EventTaskEditor";
import { confirmDiscardDraft, useBeforeUnloadGuard } from "../acquisitionDraftGuard";
import { ConnectionSelect } from "./ConnectionSelect";
import { useConnectionQuery } from "./useConnectionQuery";
import { useDriverEventCatalog } from "./useDriverEventCatalog";
import { useDraftGuardReporter } from "./draftNavGuard";
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
  // 事件能力目录（判据只有 Descriptor：`events.streams` 为空 = 不支持）。
  const eventCatalog = useDriverEventCatalog(active?.driver_id ?? null);
  // 统一的「应用内离开」守卫：侧栏 / 面包屑 / 设备 Tab / 浏览器前进后退共用一个来源。
  useDraftGuardReporter(draft.dirty);
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
          {/* 事件订阅：只有驱动真的声明了事件流才给这块区域（§2.5-8）。
              不支持 ⇒ 整块不渲染（不再显示「该驱动未声明事件流」这种常驻说明）。 */}
          {eventCatalog.state === "ready" && eventCatalog.streams.length > 0 ? (
            <div data-testid="event-subscription-region">
              <Collapse
                // 默认收起（§7-8）：事件订阅不是默认页面的一屏四块之一，展开才占空间。
                defaultActiveKey={[]}
                items={[
                  {
                    key: "event-subscription",
                    label: "事件订阅",
                    children: <EventTaskEditor fixedEndpointId={active.id} />,
                  },
                ]}
              />
            </div>
          ) : null}
          {/* 描述读取失败是**真实问题**（§3.3），不静默隐藏：给一句影响说明 + 重试入口。 */}
          {eventCatalog.state === "error" ? (
            <Alert
              type="warning"
              showIcon
              data-testid="event-catalog-error"
              message="事件订阅能力读取失败（不影响上面的数据采集配置）"
              description={eventCatalog.error}
              action={
                <button
                  type="button"
                  data-testid="retry-event-catalog"
                  onClick={eventCatalog.reload}
                  style={{ cursor: "pointer" }}
                >
                  重试
                </button>
              }
            />
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
