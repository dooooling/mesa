// M1 Connection Context：Device Workspace 连接选择的唯一真相来源。
// - 观察类 tab（overview/data/events）允许 `全部连接`（URL 无参即全部，
//   `?connection=all` 归一化为无参，避免两种“全部”写法漂移）；
// - 配置类 tab（config/diagnostics）必须单连接：优先沿用 URL 的
//   `?connection=`，无效时回落到当前设备第一个连接，并把最终选择同步回 URL；
// - 设备无连接时 effective 为 null（页面显式空状态，不得编造）。
// 本模块为纯函数，可单测；组件只负责把 normalizedParam 写回 URL（replace）。

export type WorkspaceTab = "overview" | "data" | "events" | "config" | "diagnostics";

export const WORKSPACE_TABS: WorkspaceTab[] = ["overview", "data", "events", "config", "diagnostics"];

export function isWorkspaceTab(v: string): v is WorkspaceTab {
  return (WORKSPACE_TABS as string[]).includes(v);
}

/** 配置类 tab 必须单连接（settings / acquisition / diagnostics 本就是 Endpoint 级操作）。 */
export function isSingleConnectionTab(tab: WorkspaceTab): boolean {
  return tab === "config" || tab === "diagnostics";
}

export type EffectiveConnection =
  | { kind: "all" }
  | { kind: "single"; endpointId: string }
  | { kind: "none" };

export interface ResolvedConnection {
  effective: EffectiveConnection;
  /** URL 应该是什么（null = 无参即全部）；调用方对比当前 param 决定是否 replace 同步。 */
  normalizedParam: string | null;
}

/**
 * 解析连接选择。endpointIds 为当前设备下连接 id 列表（顺序即展示顺序，
 * 第一个为回落默认，保持稳定）。
 */
export function resolveEffectiveConnection(args: {
  tab: WorkspaceTab;
  param: string | null;
  endpointIds: string[];
}): ResolvedConnection {
  const { tab, param, endpointIds } = args;
  const clean = (param ?? "").trim();
  if (isSingleConnectionTab(tab)) {
    if (clean && endpointIds.includes(clean)) {
      return { effective: { kind: "single", endpointId: clean }, normalizedParam: clean };
    }
    if (endpointIds.length > 0) {
      return { effective: { kind: "single", endpointId: endpointIds[0] }, normalizedParam: endpointIds[0] };
    }
    return { effective: { kind: "none" }, normalizedParam: null };
  }
  // 观察类 tab：无参 / all 均视为全部；合法 id 视为单连接上下文；非法回落全部。
  if (!clean || clean === "all") {
    return { effective: { kind: "all" }, normalizedParam: null };
  }
  if (endpointIds.includes(clean)) {
    return { effective: { kind: "single", endpointId: clean }, normalizedParam: clean };
  }
  return { effective: { kind: "all" }, normalizedParam: null };
}
