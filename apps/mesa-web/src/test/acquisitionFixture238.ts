// 238 点离线夹具（§5.1 / §7.2-1）。
//
// 来源与派生方式：由 `target/validation/t0-rev13-tasks.json`（rev13 的 238 点任务）
// 的**形状**派生——235 个原生点位 + 3 个语义点位，区域/模块的分布逐项对齐快照：
//   A:26×32  B:127×1  C:112×2  C:114×3  C:115×2  C:122×1  C:125×3  C:127×5
//   C:21×134 C:26×8   N:26×8   V:26×36
// 三个语义点位沿用快照的变量与 Key 形态（`@channel=i:1,axis=i:1` 这类整数前缀）。
//
// 为什么要单独抽一个夹具：238 点是本次页面重构的验收规模（分页 10 页、搜索定位、
// 第 10 页编辑后位置保持）。夹具必须**离线**（不启服务、不连真机），可机械判定。
//
// 使用约定（§5.1）：本文件只给**正常场景**（加载成功、配置完整、身份核验通过）。
// 需要「真实问题照常出现」的用例请自建问题夹具，两者不得混用。
import { NCK_POINTS_KIND, type AcquisitionTaskShape, type NckPointSelection } from "../deviceModel";
import type { NckDefinitionMap } from "../components/EndpointAcquisitionPane";
import type { ResourceDescriptor } from "../types";
import { CANDIDATE_AVAILABILITY } from "./availabilityFixture";

/** 原生点位的 (area, module, 数量) 分布（逐项对齐 238 点快照中的 235 个原生点位）。 */
export const NATIVE_DISTRIBUTION: ReadonlyArray<readonly [string, number, number]> = [
  ["A", 26, 32],
  ["B", 127, 1],
  ["C", 112, 2],
  ["C", 114, 3],
  ["C", 115, 2],
  ["C", 122, 1],
  ["C", 125, 3],
  ["C", 127, 5],
  ["C", 21, 134],
  ["C", 26, 8],
  ["N", 26, 8],
  ["V", 26, 36],
];

/** 快照里的三个语义点位（label 用目录中文名；身份模板与 Key 形态沿用快照）。 */
export const SEMANTIC_DEFINITIONS: NckDefinitionMap = {
  "axis.position.actual_bcs": {
    variable_id: "axis.position.actual_bcs",
    revision: 1,
    family: "828d",
    scope: "collect",
    label: { zh: "轴实际位置", en: "Axis actual position" },
    targets: ["axis", "channel"],
    target_required: ["axis", "channel"],
    arguments: [],
    availability: CANDIDATE_AVAILABILITY,
    identity_template: "axis.position.actual_bcs@axis=i:{axis},channel=i:{channel}",
  },
  "channel.feed.actual": {
    variable_id: "channel.feed.actual",
    revision: 1,
    family: "828d",
    scope: "collect",
    label: { zh: "进给速度", en: "Feed rate" },
    targets: ["channel"],
    target_required: ["channel"],
    arguments: [],
    availability: CANDIDATE_AVAILABILITY,
    identity_template: "channel.feed.actual@channel=i:{channel}",
  },
  "channel.program.status": {
    variable_id: "channel.program.status",
    revision: 1,
    family: "828d",
    scope: "collect",
    label: { zh: "程序状态", en: "Program status" },
    targets: ["channel"],
    target_required: ["channel"],
    arguments: [],
    availability: CANDIDATE_AVAILABILITY,
    identity_template: "channel.program.status@channel=i:{channel}",
  },
};

/** 语义点位的 Key 形态（与快照一致：整数 target 带 `i:` 前缀、键名升序）。 */
export function semanticPointKey(
  variableId: string,
  target: Record<string, number>,
): string {
  const body = Object.keys(target)
    .sort()
    .map((key) => `${key}=i:${target[key]}`)
    .join(",");
  return `${variableId}@${body}`;
}

/** 原生点位 Key（与 derived 规则同形：native.<area>.<area_no>.<module>.<column>.<line>）。 */
export function nativePointKey(params: {
  area: string;
  area_no: number;
  module: number;
  column: number;
  line: number;
}): string {
  return `native.${params.area}.${params.area_no}.${params.module}.${params.column}.${params.line}`;
}

/** Descriptor 的 `variable` 资源 Schema（宽松但完整：满足原生字段校验，不制造问题）。 */
export const NATIVE_RESOURCE: ResourceDescriptor = {
  id: "variable",
  label: { default: "NC Variable" },
  parameters: {
    fields: [
      { key: "area", label: "Area", field_type: "string", required: true, validation: {}, ui: {} },
      { key: "area_no", label: "Area No.", field_type: "integer", required: true, validation: {}, ui: {} },
      { key: "module", label: "Module", field_type: "integer", required: true, validation: {}, ui: {} },
      { key: "column", label: "Column", field_type: "integer", required: true, validation: {}, ui: {} },
      { key: "line", label: "Line", field_type: "integer", required: true, validation: {}, ui: {} },
      { key: "count", label: "Count", field_type: "integer", required: false, validation: {}, ui: {} },
      { key: "unit_mode", label: "Unit mode", field_type: "string", required: false, validation: {}, ui: {} },
      { key: "decode", label: "Decode", field_type: "string", required: false, validation: {}, ui: {} },
      { key: "transport_size", label: "Transport", field_type: "string", required: false, validation: {}, ui: {} },
    ],
  },
  outputs: [],
  modes: ["poll"],
};

export interface AcquisitionFixture238 {
  points: NckPointSelection[];
  /** 一个 poll 任务（interval_ms = 1000），与快照同 kind。 */
  task: AcquisitionTaskShape;
  definitions: NckDefinitionMap;
  resource: ResourceDescriptor;
}

/**
 * 生成 238 点夹具（235 原生 + 3 语义），顺序与快照一致：原生在前、语义在后。
 *
 * 原生地址用 (area, module, 序号) 唯一化：column = 序号、line = 1，因此同一
 * (area, module) 下的多个点位在列表里仍可区分（也保证 derived Key 唯一）。
 */
export function buildAcquisition238(intervalMs = 1000): AcquisitionFixture238 {
  const points: NckPointSelection[] = [];
  for (const [area, module, count] of NATIVE_DISTRIBUTION) {
    for (let i = 1; i <= count; i += 1) {
      const parameters: Record<string, unknown> = {
        area,
        area_no: 1,
        module,
        column: i,
        line: 1,
        count: 1,
        unit_mode: "current",
        decode: "F64_LE",
        transport_size: "0x09",
      };
      points.push({
        kind: "nck_native",
        point_key: nativePointKey({ area, area_no: 1, module, column: i, line: 1 }),
        parameters,
      });
    }
  }
  const semantic: Array<{ variableId: string; target: Record<string, number> }> = [
    { variableId: "axis.position.actual_bcs", target: { axis: 1, channel: 1 } },
    { variableId: "channel.feed.actual", target: { channel: 1 } },
    { variableId: "channel.program.status", target: { channel: 1 } },
  ];
  for (const item of semantic) {
    points.push({
      kind: "nck_variable",
      point_key: semanticPointKey(item.variableId, item.target),
      parameters: {
        variable_id: item.variableId,
        definition_revision: 1,
        target: item.target,
        arguments: {},
      },
    });
  }
  const task: AcquisitionTaskShape = {
    id: "nck-task",
    schedule: { mode: "poll", interval_ms: intervalMs },
    binding: { kind: NCK_POINTS_KIND, config: { points } },
  };
  return { points, task, definitions: SEMANTIC_DEFINITIONS, resource: NATIVE_RESOURCE };
}
