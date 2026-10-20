// PR-D1 第二轮复审：统计/导出分析作用域 = 设备 + 连接（双重）。
//
// 为什么必须设备也参与：表格只显示这台机床的数据，旁边统计却包含其他设备，
// 不符合现场监控预期。即使写"跨设备"提示也不如让范围一致。
//
// 边界（逐条断言，防止实现漂移）：
// 1. 质量状态筛选（GOOD/BAD/STALE）与搜索关键字**不进入**本作用域；
// 2. 归属不可证明的点（端点不在清单里）在设备筛选下**必须排除**（fail-closed），
//    绝不把"无法判断"解释成"可能属于所选设备"。
import { describe, expect, it } from "vitest";
import {
  filterPointsInQualityScope,
  makeQualityScope,
  pointInQualityScope,
  qualityScopeDescription,
  qualityScopeLabel,
  qualityScopeText,
} from "./qualityScope";

/** 两台设备、各两个连接。 */
const DEVICE_OF = new Map<string, string>([
  ["ep-a1", "dev-a"],
  ["ep-a2", "dev-a"],
  ["ep-b1", "dev-b"],
  ["ep-b2", "dev-b"],
]);
const deviceOf = (endpointId: string) => DEVICE_OF.get(endpointId) ?? "";

/** 4 个可归属点 + 1 个归属不可证明点（ghost）。 */
const POINTS = [
  { endpoint_id: "ep-a1" },
  { endpoint_id: "ep-a2" },
  { endpoint_id: "ep-b1" },
  { endpoint_id: "ep-b2" },
  { endpoint_id: "ghost" },
];

describe("qualityScope · 设备 + 连接双重作用域", () => {
  it("scope_matrix_four_combinations", () => {
    // 四种组合的**唯一真值**：全部设备/全部连接 → 该设备全部连接 →
    // 该设备单连接 → 全部设备单连接。
    const cases: Array<{
      deviceId: string | null;
      endpointId: string | null;
      rows: number;
      endpoints: string[];
    }> = [
      { deviceId: null, endpointId: null, rows: 5, endpoints: ["ep-a1", "ep-a2", "ep-b1", "ep-b2", "ghost"] },
      { deviceId: "dev-a", endpointId: null, rows: 2, endpoints: ["ep-a1", "ep-a2"] },
      { deviceId: "dev-a", endpointId: "ep-a1", rows: 1, endpoints: ["ep-a1"] },
      { deviceId: null, endpointId: "ep-a1", rows: 1, endpoints: ["ep-a1"] },
    ];
    for (const c of cases) {
      const scope = makeQualityScope(c.deviceId, c.endpointId);
      const got = filterPointsInQualityScope(POINTS, scope, deviceOf);
      expect(got.map((p) => p.endpoint_id)).toEqual(c.endpoints);
      expect(got).toHaveLength(c.rows);
    }
  });

  it("device_selected_connection_all_is_device_only_not_cross_device", () => {
    // 设备选中 + 连接 ALL：范围收敛到该设备的全部连接，**不是**跨设备。
    const scope = makeQualityScope("dev-a", "ALL");
    expect(scope).toEqual({ deviceId: "dev-a", endpointId: null });
    const got = filterPointsInQualityScope(POINTS, scope, deviceOf);
    expect(got.every((p) => deviceOf(p.endpoint_id) === "dev-a")).toBe(true);
    // 另一台设备的点一个都不能进。
    expect(got.some((p) => p.endpoint_id === "ep-b1")).toBe(false);
  });

  it("unattributed_points_fail_closed_when_device_selected", () => {
    // 端点不在 /endpoints 清单里 ⇒ 归属不可证明 ⇒ 设备筛选下排除。
    const scope = makeQualityScope("dev-a", null);
    expect(pointInQualityScope({ endpoint_id: "ghost" }, scope, deviceOf)).toBe(false);
    // 未选设备时 ghost 仍在（聚合本职；表格以"归属列"如实展示）。
    expect(pointInQualityScope({ endpoint_id: "ghost" }, makeQualityScope(), deviceOf)).toBe(true);
  });

  it("point_level_device_id_wins_over_endpoint_map", () => {
    // UI 行已解析出 deviceId 时以它为准（端点映射只是导出路径的回退）。
    const scope = makeQualityScope("dev-a", null);
    expect(
      pointInQualityScope({ endpoint_id: "ep-b1", deviceId: "dev-a" }, scope, deviceOf),
    ).toBe(true);
    expect(
      pointInQualityScope({ endpoint_id: "ep-a1", deviceId: "dev-b" }, scope, deviceOf),
    ).toBe(false);
  });

  it("scope_labels_and_description_reflect_real_scope", () => {
    // 设备 + 连接 ALL：文案必须写"该设备 + 全部连接"，不得再出现"跨设备"。
    const deviceAll = makeQualityScope("dev-a", null);
    expect(qualityScopeText(deviceAll, { device: "CNC-A" })).toBe("CNC-A · 全部连接");
    expect(qualityScopeLabel(deviceAll, { device: "CNC-A" })).toBe("统计范围：CNC-A · 全部连接");
    expect(qualityScopeDescription(deviceAll)).toContain("dev-a");
    expect(qualityScopeDescription(deviceAll)).not.toContain("跨设备");

    // 设备 + 单连接：两端都写实。
    const both = makeQualityScope("dev-a", "ep-a1");
    expect(qualityScopeText(both, { device: "CNC-A", endpoint: "NCK-1" })).toBe("CNC-A · NCK-1");
    expect(qualityScopeDescription(both)).toContain("ep-a1");

    // 全部设备 + 单连接。
    const connOnly = makeQualityScope(null, "ep-a1");
    expect(qualityScopeText(connOnly)).toBe("全部设备 · ep-a1");
    expect(qualityScopeDescription(connOnly)).toContain("全部设备");

    // 全部设备 + 全部连接。
    expect(qualityScopeText(makeQualityScope())).toBe("全部设备 · 全部连接");
    expect(qualityScopeDescription(makeQualityScope())).toContain("全部连接");
  });

  it("status_and_search_are_not_part_of_scope", () => {
    // 作用域函数只认 endpoint_id / deviceId 两个字段；质量状态与搜索词
    // 不是它的输入（类型层面也不接受），因此不会缩小基础统计范围。
    const scope = makeQualityScope("dev-a", null);
    const points = [
      { endpoint_id: "ep-a1", derived: "GOOD", displayKey: "spindle.speed" },
      { endpoint_id: "ep-a2", derived: "BAD", displayKey: "channel.1" },
    ];
    expect(filterPointsInQualityScope(points, scope, deviceOf)).toHaveLength(2);
  });
});
