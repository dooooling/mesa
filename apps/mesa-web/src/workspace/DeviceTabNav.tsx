// 设备一级 Tab 导航：概览 | 实时数据 | 连接 | 采集 | 事件 | 诊断。
// 无 config，无嵌套。Tab 切换不保留 ?connection=（各页连接筛选局部化）。
//
// 离开确认不再由本组件自己实现：Tab 与侧栏、面包屑、浏览器前进后退共用
// `draftNavGuard` 的**同一个守卫**（有未保存草稿时先确认，取消则本次跳转不发生，
// §7-7）。本组件只负责「把点击交给守卫」，不再持有第二套判定。
import { Link } from "react-router-dom";
import { Tabs } from "antd";
import { DRAFT_LEAVE_ACTION, useGuardedNavigate } from "./draftNavGuard";

export const DEVICE_TABS = [
  { key: "overview", label: "概览" },
  { key: "data", label: "实时数据" },
  { key: "connections", label: "连接" },
  { key: "acquisition", label: "采集" },
  { key: "events", label: "事件" },
  { key: "diagnostics", label: "诊断" },
] as const;

export type DeviceTabKey = (typeof DEVICE_TABS)[number]["key"];

export function isDeviceTab(v: string): v is DeviceTabKey {
  return (DEVICE_TABS as readonly { key: string }[]).some((t) => t.key === v);
}

export function DeviceTabNav(props: { deviceId: string; active: string }) {
  const { deviceId, active } = props;
  const guardedNavigate = useGuardedNavigate();
  return (
    <Tabs
      data-testid="device-tab-nav"
      activeKey={isDeviceTab(active) ? active : "overview"}
      items={DEVICE_TABS.map((t) => ({
        key: t.key,
        label: (
          <Link
            to={`/devices/${deviceId}/${t.key}`}
            onClick={(event) => {
              // 统一守卫：不直接导航，先问「要不要放弃草稿」，取消则停在原页。
              event.preventDefault();
              guardedNavigate(`/devices/${deviceId}/${t.key}`, DRAFT_LEAVE_ACTION);
            }}
          >
            {t.label}
          </Link>
        ),
      }))}
    />
  );
}
