// 设备一级 Tab 导航：概览 | 实时数据 | 连接 | 采集 | 事件 | 诊断。
// 无 config，无嵌套。Tab 切换不保留 ?connection=（各页连接筛选局部化）。
//
// `beforeNavigate`：离开当前 Tab 前的一次确认（采集页用它提示未保存修改，
// 避免切走就静默丢掉草稿，§7-7）。返回 false 则阻止本次跳转。
import { Link, useNavigate } from "react-router-dom";
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

export function DeviceTabNav(props: {
  deviceId: string;
  active: string;
  /** 返回 false 阻止跳转（异步确认：有未保存修改时先问一次）。 */
  beforeNavigate?: (nextKey: string) => boolean | Promise<boolean>;
}) {
  const { deviceId, active, beforeNavigate } = props;
  const nav = useNavigate();
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
              if (!beforeNavigate) return;
              event.preventDefault();
              void (async () => {
                if (await beforeNavigate(t.key)) nav(`/devices/${deviceId}/${t.key}`);
              })();
            }}
          >
            {t.label}
          </Link>
        ),
      }))}
    />
  );
}
