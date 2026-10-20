// PR-D1 第二轮复审：质量统计 / 按码分解 / 导出分析的**唯一作用域真值**。
//
// 为什么必须单一函数：计数行、Popover、导出 `analysis_scope` 过去各算一遍，
// 出现"表格只显示这台机床，旁边统计却含其他设备"的口径分裂。现在三者都调用
// 本模块（设备 + 连接双重作用域），范围一致，符合现场监控预期。
//
// 两条硬边界：
// 1. **质量状态筛选（GOOD/BAD/STALE）与搜索关键字不进入本作用域**：否则用户
//    搜一个点后会误以为整台设备只有一个点。它们只影响表格行。
// 2. **归属不可证明的点在设备筛选下排除（fail-closed）**：端点不在
//    `/api/v1/endpoints` 清单里 ⇒ 无法证明它属于所选设备，绝不把"无法判断"
//    解释成"可能属于所选设备"（与表格 RC2 修1 同口径）。
//
// 零后端改动：`/endpoints` 响应已含 `device_id`，设备归属在前端按该字段过滤，
// 不新增接口、不突破 4 次 GET 上界。

/** 统计/分析作用域：`null` 表示该维度不筛选（全部）。 */
export interface QualityScope {
  /** 设备 id；`null` = 全部设备。 */
  deviceId: string | null;
  /** 连接（端点）id；`null` = 全部连接。 */
  endpointId: string | null;
}

/** 作用域里两个维度的可读名（设备名 / 连接名）；缺省回落到 id 或"全部…"。 */
export interface QualityScopeLabels {
  device?: string;
  endpoint?: string;
}

/** 参与作用域判断的最小点形状（UI 行与导出原始行都能满足）。 */
export interface ScopedPoint {
  endpoint_id: string;
  /**
   * 点已解析出的设备归属（UI 行有；导出原始行没有，由端点映射补齐）。
   * 空串视为"未知"，回落到端点映射，仍未知则 fail-closed。
   */
  deviceId?: string;
}

export const ALL_DEVICES_LABEL = "全部设备";
export const ALL_CONNECTIONS_LABEL = "全部连接";

/**
 * 归一化外部输入（URL query / 组件 props）为作用域。
 * `"ALL"`、空串、`undefined` 一律视为"不筛选"，避免出现"筛选到字面量 ALL"。
 */
export function makeQualityScope(
  deviceId?: string | null,
  endpointId?: string | null,
): QualityScope {
  return {
    deviceId: deviceId && deviceId !== "ALL" ? deviceId : null,
    endpointId: endpointId && endpointId !== "ALL" ? endpointId : null,
  };
}

/**
 * 单点是否在作用域内。
 *
 * `deviceOfEndpoint` 是端点 → 设备的映射查询（导出路径用它补齐原始行没有的
 * `device_id`）；UI 行自带 `deviceId` 时以行为准。
 */
export function pointInQualityScope(
  point: ScopedPoint,
  scope: QualityScope,
  deviceOfEndpoint: (endpointId: string) => string,
): boolean {
  if (scope.endpointId !== null && point.endpoint_id !== scope.endpointId) return false;
  if (scope.deviceId === null) return true;
  const pointDeviceId =
    point.deviceId && point.deviceId.length > 0
      ? point.deviceId
      : deviceOfEndpoint(point.endpoint_id);
  return pointDeviceId === scope.deviceId;
}

/** 作用域过滤（保持输入顺序；不改写点对象）。 */
export function filterPointsInQualityScope<T extends ScopedPoint>(
  points: readonly T[],
  scope: QualityScope,
  deviceOfEndpoint: (endpointId: string) => string,
): T[] {
  return points.filter((p) => pointInQualityScope(p, scope, deviceOfEndpoint));
}

/** 作用域的可读文本，如 `CNC-01 · 全部连接`（不含"统计范围"前缀）。 */
export function qualityScopeText(scope: QualityScope, labels: QualityScopeLabels = {}): string {
  const device =
    scope.deviceId === null ? ALL_DEVICES_LABEL : labels.device || scope.deviceId;
  const endpoint =
    scope.endpointId === null ? ALL_CONNECTIONS_LABEL : labels.endpoint || scope.endpointId;
  return `${device} · ${endpoint}`;
}

/** 计数行旁的范围标注（写实，不再出现"跨设备"这种与真实范围不符的提示）。 */
export function qualityScopeLabel(scope: QualityScope, labels: QualityScopeLabels = {}): string {
  return `统计范围：${qualityScopeText(scope, labels)}`;
}

/**
 * 导出 `analysis_scope.description` 的文案（导出侧只有 id，没有显示名）。
 * 四种组合必须各自写实，便于消费方在不解析 id 的情况下也能读懂范围。
 */
export function qualityScopeDescription(scope: QualityScope): string {
  if (scope.deviceId === null && scope.endpointId === null) {
    return "分析覆盖全部设备与全部连接（未按设备/连接筛选）";
  }
  if (scope.endpointId === null) {
    return `分析仅覆盖设备 ${scope.deviceId} 的全部连接（按设备筛选，范围收敛到该设备）`;
  }
  if (scope.deviceId === null) {
    return `分析仅覆盖所选连接 ${scope.endpointId}（全部设备）`;
  }
  return `分析仅覆盖设备 ${scope.deviceId} 的连接 ${scope.endpointId}`;
}
