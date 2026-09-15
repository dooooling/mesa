// 设备一级 Tab 导航：概览 | 实时数据 | 连接 | 采集 | 事件 | 诊断。
// 无 config，无嵌套。Tab 切换不保留 ?connection=（各页连接筛选局部化）。
import { Link } from "react-router-dom";
import { Tabs } from "antd";

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
  return (
    <Tabs
      activeKey={isDeviceTab(active) ? active : "overview"}
      items={DEVICE_TABS.map((t) => ({
        key: t.key,
        label: <Link to={`/devices/${deviceId}/${t.key}`}>{t.label}</Link>,
      }))}
    />
  );
}
