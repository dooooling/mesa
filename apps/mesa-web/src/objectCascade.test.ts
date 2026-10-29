// 联动选择判定层（`objectCascade.ts`）的纯模型门禁。
//
// 这里只钉三件在产品上真正会出事的事实：
// 1. **参数只能来自驱动载荷**：候选的归属与可提供的键都读 wire 字段，
//    前端不得从 id/标签推断（否则会挑到别的变量的实例）；
// 2. **"探测失败"与"设备没有"必须区分**：分支不可达只能承认探测不完整；
// 3. **缺参数就是缺参数**：探测不可用 / 未探测 / 失败 / 无候选 / 未选择
//    五种原因分别有稳定码与不同文案，任何一种都不得退化成"填个默认值"。
import { describe, expect, it } from "vitest";
import type { BrowseNode } from "./browseModel";
import type { NckFormModel } from "./nckPointModel";
import type { ObjectNeed } from "./objectProbe";
import {
  cascadeGuidance,
  cascadeKeysOf,
  cascadeOptionFromNode,
  describeMissingParam,
  evaluateCascadeReadiness,
  isKeyResolved,
  nodeBelongsToVariable,
  optionsForKey,
  scanObjectCandidates,
  type CascadeKey,
  type CascadeScan,
} from "./objectCascade";

const VARIABLE = "channel.feed.actual";

function model(overrides: Partial<NckFormModel> = {}): NckFormModel {
  return {
    ok: true,
    targetFields: [
      {
        kind: "channel",
        required: true,
        requiresEntry: false,
        entryDomainDeclared: false,
        entryExamples: [],
      },
    ],
    argumentFields: [],
    ...overrides,
  } as NckFormModel;
}

const CHANNEL_NEED: ObjectNeed = {
  kind: "channel",
  required: true,
  source: "definition_targets",
};

const DEFINITION = {
  variable_id: VARIABLE,
  revision: 1,
  targets: ["channel"],
  target_required: ["channel"],
};

/** 一个语义可选节点（载荷来自驱动的 `selection_payload_json` 形态）。 */
function semanticNode(args: {
  id: string;
  label?: string;
  target?: Record<string, unknown>;
  arguments?: Record<string, unknown>;
  variableId?: string;
  appliesTo?: string[];
  keys?: string[];
  hasChildren?: boolean;
}): BrowseNode {
  return {
    id: args.id,
    label: args.label ?? args.id,
    kind: "nck_variable",
    has_children: args.hasChildren ?? false,
    selection_mode: 2,
    selection_payload_json: JSON.stringify({
      identity: {
        applies_to_variables: args.appliesTo ?? [],
        keys: (args.keys ?? ["channel"]).map((path) => ({ path, required: true })),
      },
      kind: "nck_variable",
      parameters: {
        variable_id: args.variableId,
        target: args.target ?? {},
        arguments: args.arguments ?? {},
      },
    }),
  };
}

describe("级联键：只取必填参数，顺序与定义一致", () => {
  it("required_targets_then_required_arguments_with_entry_subkey", () => {
    const keys = cascadeKeysOf(
      model({
        targetFields: [
          {
            kind: "channel",
            required: true,
            requiresEntry: false,
            entryDomainDeclared: false,
            entryExamples: [],
          },
          {
            kind: "spindle",
            required: false,
            requiresEntry: false,
            entryDomainDeclared: false,
            entryExamples: [],
          },
          {
            kind: "drive_object",
            required: true,
            requiresEntry: true,
            entryDomainDeclared: false,
            entryExamples: ["nc_variable"],
          },
        ],
        argumentFields: [
          { key: "index", label: "主轴号", valueType: "integer", required: true, enumOptions: [] },
          { key: "opt", label: "可选", valueType: "integer", required: false, enumOptions: [] },
        ],
      }),
    );
    expect(keys).toEqual([
      { key: "channel", label: "channel", source: "target", subKey: undefined },
      { key: "drive_object", label: "drive_object", source: "target", subKey: "drive_object.entry" },
      { key: "index", label: "主轴号", source: "argument" },
    ]);
  });

  it("entry_subkey_must_also_be_present_for_a_drive_object_level", () => {
    const key: CascadeKey = {
      key: "drive_object",
      label: "drive_object",
      source: "target",
      subKey: "drive_object.entry",
    };
    expect(isKeyResolved(key, { target: { drive_object: "26" }, arguments: {} })).toBe(false);
    expect(
      isKeyResolved(key, {
        target: { drive_object: "26", "drive_object.entry": "nc_variable" },
        arguments: {},
      }),
    ).toBe(true);
  });
});

describe("候选归属：只认 wire 声明，不从 id/标签推断", () => {
  const payload = (parameters: Record<string, unknown>, applies?: unknown) => ({
    parameters,
    identity: applies === undefined ? {} : { applies_to_variables: applies },
  });

  it("explicit_variable_id_must_match", () => {
    expect(payloadBelongs(payload({ variable_id: VARIABLE }))).toBe(true);
    expect(payloadBelongs(payload({ variable_id: "spindle.speed.actual" }))).toBe(false);
  });

  it("applies_to_variables_filters_other_variables", () => {
    expect(payloadBelongs(payload({}, [VARIABLE]))).toBe(true);
    expect(payloadBelongs(payload({}, ["spindle.speed.actual"]))).toBe(false);
    // 空数组 = 没声明适用范围（不是"什么都不适用"）：按通用实例提供者处理。
    expect(payloadBelongs(payload({}, []))).toBe(true);
  });

  function payloadBelongs(value: Record<string, unknown>): boolean {
    return nodeBelongsToVariable(value, VARIABLE);
  }
});

describe("节点 → 候选：可用性由驱动载荷决定", () => {
  it("selectable_node_yields_fill_with_provided_keys", () => {
    const mapped = cascadeOptionFromNode({
      node: semanticNode({ id: "n1", target: { channel: 2 } }),
      definition: DEFINITION,
      needs: [CHANNEL_NEED],
    });
    expect(mapped && "option" in mapped).toBe(true);
    const option = (mapped as { option: { provides: string[]; fill: unknown } }).option;
    expect(option.provides).toEqual(["channel"]);
    expect(option.fill).toMatchObject({ target: { channel: "2" } });
  });

  it("not_selectable_node_is_kept_as_blocked_option_not_dropped", () => {
    const node: BrowseNode = {
      ...semanticNode({ id: "n2", target: { channel: 1 } }),
      selection_mode: 3,
    };
    const mapped = cascadeOptionFromNode({
      node,
      definition: DEFINITION,
      needs: [CHANNEL_NEED],
    });
    const option = (mapped as { option: { blocked: { code: string } } }).option;
    expect(option.blocked.code).toBe("OBJECT_NODE_NOT_SELECTABLE");
  });

  it("payload_without_values_is_blocked_with_address_mapping_code", () => {
    const mapped = cascadeOptionFromNode({
      node: semanticNode({ id: "n3", variableId: VARIABLE, target: {}, keys: ["mode_group"] }),
      definition: { ...DEFINITION, targets: ["mode_group"], target_required: ["mode_group"] },
      needs: [{ kind: "mode_group", required: true, source: "definition_targets" }],
    });
    const option = (mapped as { option: { blocked: { code: string } } }).option;
    expect(option.blocked.code).toBe("OBJECT_NO_ADDRESS_MAPPING");
  });

  it("node_of_another_variable_is_not_offered", () => {
    expect(
      cascadeOptionFromNode({
        node: semanticNode({ id: "n4", variableId: "spindle.speed.actual", target: { channel: 1 } }),
        definition: DEFINITION,
        needs: [CHANNEL_NEED],
      }),
    ).toBeNull();
  });

  it("driver_reported_anomaly_without_payload_stays_visible_as_diagnostic", () => {
    const mapped = cascadeOptionFromNode({
      node: {
        id: "n5",
        label: "通道3 无法判定",
        kind: "diagnostic",
        has_children: false,
        selection_mode: 3,
        selection_payload_json: "",
        diagnostics: [{ code: "IDENTITY_PENDING_REVIEW", message: "无法判定", severity: 2 }],
      },
      definition: DEFINITION,
      needs: [CHANNEL_NEED],
    });
    expect(mapped && "diagnostic" in mapped).toBe(true);
  });

  it("branch_node_is_not_a_candidate", () => {
    expect(
      cascadeOptionFromNode({
        node: semanticNode({ id: "n6", hasChildren: true }),
        definition: DEFINITION,
        needs: [CHANNEL_NEED],
      }),
    ).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// 探测：广度优先 + 分支失败逐条记录 + 分页
// ---------------------------------------------------------------------------

interface PageSpec {
  nodes: BrowseNode[];
  next_cursor?: string | null;
}

function fakeFetch(pages: Record<string, PageSpec | "fail">, failCode = "CONNECTION_LOST") {
  const calls: string[] = [];
  const impl = async (input: string, init?: RequestInit) => {
    const body = JSON.parse(String(init?.body ?? "{}")) as { parent?: string; cursor?: string };
    const parent = body.parent ?? "";
    const cursor = body.cursor ?? "";
    calls.push(parent);
    const spec = pages[parent];
    if (spec === undefined || spec === "fail") {
      return {
        ok: false,
        status: 400,
        json: async () => ({
          error: { code: failCode, message: `未知的浏览父节点 ${parent}` },
        }),
      };
    }
    // 第二页（cursor 非空）恒为空且不再给游标：与驱动"分页到底"的行为一致。
    if (cursor !== "") {
      return { ok: true, status: 200, json: async () => ({ nodes: [], next_cursor: null }) };
    }
    return {
      ok: true,
      status: 200,
      json: async () => ({ nodes: spec.nodes, next_cursor: spec.next_cursor ?? null }),
    };
  };
  return { impl, calls };
}

const CHANNELS_ROOT: BrowseNode = {
  id: "tree/channels",
  label: "通道",
  kind: "folder",
  has_children: true,
  selection_mode: 3,
  selection_payload_json: "",
};

async function scanFixture(pages: Record<string, PageSpec | "fail">) {
  const { impl, calls } = fakeFetch(pages);
  const scan = await scanObjectCandidates({
    endpointId: "ep-1",
    definition: DEFINITION,
    needs: [CHANNEL_NEED],
    fetchImpl: impl,
  });
  return { scan, calls };
}

describe("探测扫描：广度优先、失败逐条记录、不抛异常", () => {
  it("walks_folders_breadth_first_and_collects_options", async () => {
    const { scan, calls } = await scanFixture({
      "": { nodes: [{ ...CHANNELS_ROOT, id: "tree/a" }, { ...CHANNELS_ROOT, id: "tree/b" }] },
      "tree/a": { nodes: [] },
      "tree/b": { nodes: [semanticNode({ id: "b/1", target: { channel: 1 } })] },
    });
    expect(calls).toEqual(["", "tree/a", "tree/b"]);
    expect(scan.options.map((option) => option.nodeId)).toEqual(["b/1"]);
    expect(scan.incomplete).toBe(false);
  });

  it("failed_branch_is_recorded_and_does_not_abort_the_scan", async () => {
    const { scan } = await scanFixture({
      "": { nodes: [{ ...CHANNELS_ROOT, id: "tree/a" }, { ...CHANNELS_ROOT, id: "tree/b" }] },
      "tree/a": "fail",
      "tree/b": { nodes: [semanticNode({ id: "b/1", target: { channel: 1 } })] },
    });
    // 一棵子树不可达 ⇒ 探测不完整，但另一棵子树的结果仍然可用。
    expect(scan.incomplete).toBe(true);
    expect(scan.branches.filter((branch) => !branch.ok).map((branch) => branch.parent)).toEqual([
      "tree/a",
    ]);
    expect(scan.options).toHaveLength(1);
  });

  it("stops_early_when_all_required_keys_are_covered_but_keeps_prior_failures", async () => {
    const { scan, calls } = await scanFixture({
      "": {
        nodes: [
          { ...CHANNELS_ROOT, id: "tree/failing" },
          { ...CHANNELS_ROOT, id: "tree/a" },
          { ...CHANNELS_ROOT, id: "tree/b" },
        ],
      },
      // 先访问的失败分支必须保留；后面的 tree/b 不应再被访问。
      "tree/failing": "fail",
      "tree/a": {
        nodes: [
          semanticNode({ id: "a/1", target: { channel: 1 } }),
          {
            id: "a/note",
            label: "通道1 无法判定",
            kind: "diagnostic",
            has_children: false,
            selection_mode: 3,
            selection_payload_json: "",
            diagnostics: [{ code: "IDENTITY_PENDING_REVIEW", message: "无法判定", severity: 2 }],
          },
        ],
      },
      "tree/b": { nodes: [semanticNode({ id: "b/1", target: { channel: 2 } })] },
    });
    expect(calls).toEqual(["", "tree/failing", "tree/a"]);
    expect(scan.options.map((option) => option.nodeId)).toEqual(["a/1"]);
    // 提前停止不等于吞掉失败/诊断：已发现的失败分支与诊断仍在。
    expect(scan.branches.filter((branch) => !branch.ok).map((branch) => branch.parent)).toEqual([
      "tree/failing",
    ]);
    expect(scan.diagnostics.map((item) => item.code)).toEqual(["IDENTITY_PENDING_REVIEW"]);
    expect(scan.incomplete).toBe(true);
  });

  it("does_not_stop_early_when_a_required_key_is_not_covered", async () => {
    const { scan, calls } = await scanFixture({
      "": { nodes: [{ ...CHANNELS_ROOT, id: "tree/a" }, { ...CHANNELS_ROOT, id: "tree/b" }] },
      "tree/a": { nodes: [] },
      "tree/b": { nodes: [] },
    });
    // 没有候选 ⇒ 必须把两棵子树都访问完，不能因为"访问过了"就提前收工。
    expect(calls).toEqual(["", "tree/a", "tree/b"]);
    expect(scan.options).toHaveLength(0);
  });

  it("pagination_is_followed_for_the_same_parent", async () => {
    const { scan, calls } = await scanFixture({
      "": { nodes: [{ ...CHANNELS_ROOT, id: "tree/a" }], next_cursor: "c1" },
      "tree/a": { nodes: [semanticNode({ id: "a/1", target: { channel: 1 } })] },
    });
    expect(calls).toEqual(["", "", "tree/a"]);
    expect(scan.options.map((option) => option.nodeId)).toEqual(["a/1"]);
  });

  it("driver_reported_anomalies_are_kept_as_diagnostics", async () => {
    const { scan } = await scanFixture({
      "": {
        nodes: [
          {
            id: "note/3",
            label: "通道3 无法判定",
            kind: "diagnostic",
            has_children: false,
            selection_mode: 3,
            selection_payload_json: "",
            diagnostics: [{ code: "IDENTITY_PENDING_REVIEW", message: "无法判定", severity: 2 }],
          },
        ],
      },
    });
    expect(scan.diagnostics.map((item) => item.code)).toEqual(["IDENTITY_PENDING_REVIEW"]);
  });
});

// ---------------------------------------------------------------------------
// 缺参数判定：五种原因，文案两两可区分
// ---------------------------------------------------------------------------

const KEY: CascadeKey = { key: "channel", label: "channel", source: "target" };

function scanOf(args: {
  options?: CascadeScan["options"];
  branches?: CascadeScan["branches"];
}): CascadeScan {
  const branches = args.branches ?? [{ parent: "", ok: true, code: "", message: "" }];
  return {
    options: args.options ?? [],
    diagnostics: [],
    branches,
    incomplete: branches.some((branch) => !branch.ok),
  };
}

const EMPTY_DRAFT = { target: { channel: "" }, arguments: {} };

describe("缺参数判定：原因稳定、文案可区分、绝不补默认值", () => {
  it("probe_unavailable_when_endpoint_gate_closed", () => {
    const readiness = evaluateCascadeReadiness({
      keys: [KEY],
      draft: EMPTY_DRAFT,
      scan: null,
      gate: { available: false, code: "ENDPOINT_MISSING", reason: "需先创建并连接端点" },
    });
    expect(readiness.ok).toBe(false);
    expect(readiness.missing[0]!.reason).toBe("probe_unavailable");
    expect(describeMissingParam(readiness.missing[0]!)).toContain("无法从设备探测获得");
    expect(describeMissingParam(readiness.missing[0]!)).toContain("需先创建并连接端点");
  });

  it("not_probed_before_the_dropdown_is_opened", () => {
    const readiness = evaluateCascadeReadiness({
      keys: [KEY],
      draft: EMPTY_DRAFT,
      scan: null,
      gate: { available: true },
    });
    expect(readiness.missing[0]!.reason).toBe("not_probed");
    expect(readiness.unobtainable).toHaveLength(0);
  });

  it("not_selected_when_candidates_exist", () => {
    const scan = scanOf({
      options: [
        {
          nodeId: "n1",
          label: "通道1",
          kind: "nck_variable",
          declared: ["channel"],
          provides: ["channel"],
          fill: { variableId: VARIABLE, target: { channel: "1" }, arguments: {}, resolved: ["channel"], unresolved: [], basis: "" },
          blocked: null,
        },
      ],
    });
    const readiness = evaluateCascadeReadiness({
      keys: [KEY],
      draft: EMPTY_DRAFT,
      scan,
      gate: { available: true },
    });
    expect(readiness.missing[0]!.reason).toBe("not_selected");
    expect(readiness.unobtainable).toHaveLength(0);
  });

  it("no_candidates_and_probe_failed_use_different_wording", () => {
    const noCandidates = evaluateCascadeReadiness({
      keys: [KEY],
      draft: EMPTY_DRAFT,
      scan: scanOf({}),
      gate: { available: true },
    });
    const failed = evaluateCascadeReadiness({
      keys: [KEY],
      draft: EMPTY_DRAFT,
      scan: scanOf({
        branches: [{ parent: "tree/axes", ok: false, code: "PROBE_REQUEST_FAILED", message: "不可达" }],
      }),
      gate: { available: true },
    });
    expect(noCandidates.missing[0]!.reason).toBe("no_candidates");
    expect(failed.missing[0]!.reason).toBe("probe_failed");
    const noText = describeMissingParam(noCandidates.missing[0]!);
    const failedText = describeMissingParam(failed.missing[0]!);
    expect(noText).not.toBe(failedText);
    // "设备没有"的口径不得声称探测失败；"探测失败"的口径不得声称设备没有。
    expect(noText).toContain("探测成功，但设备没有返回");
    expect(failedText).toContain("探测失败，不代表设备上没有该对象");
    expect(failedText).toContain("tree/axes");
    expect(readinessUnobtainable(failed)).toBe(true);
  });

  it("resolved_key_is_not_reported_and_never_gets_a_default", () => {
    const readiness = evaluateCascadeReadiness({
      keys: [KEY],
      draft: { target: { channel: "2" }, arguments: {} },
      scan: scanOf({}),
      gate: { available: true },
    });
    expect(readiness.ok).toBe(true);
    expect(readiness.missing).toHaveLength(0);
  });
});

function readinessUnobtainable(readiness: { unobtainable: unknown[] }): boolean {
  return readiness.unobtainable.length > 0;
}

describe("阻断指引：必须给出两条真实出路", () => {
  it("unavailable_gate_guides_to_endpoint_page_and_native_config", () => {
    const text = cascadeGuidance({
      gate: { available: false, code: "ENDPOINT_MISSING", reason: "需先创建并连接端点" },
      nativeAvailable: true,
    });
    expect(text).toContain("先完成设备创建与连接");
    expect(text).toContain("「采集」页配置点位");
    expect(text).toContain("「高级原生配置」");
  });

  it("available_gate_still_points_at_native_config_when_device_has_no_object", () => {
    const text = cascadeGuidance({ gate: { available: true }, nativeAvailable: true });
    expect(text).toContain("请先确认端点连接与设备状态后重试");
    expect(text).toContain("「高级原生配置」");
  });
});

describe("按级取候选：可用与不可用分开且都可见", () => {
  it("blocked_option_is_listed_under_its_declared_key", () => {
    const scan = scanOf({
      options: [
        {
          nodeId: "ok/1",
          label: "通道1",
          kind: "nck_variable",
          declared: ["channel"],
          provides: ["channel"],
          fill: { variableId: VARIABLE, target: { channel: "1" }, arguments: {}, resolved: ["channel"], unresolved: [], basis: "" },
          blocked: null,
        },
        {
          nodeId: "bad/3",
          label: "通道3 无法判定",
          kind: "nck_variable",
          declared: ["channel"],
          provides: [],
          fill: null,
          blocked: { code: "OBJECT_NODE_NOT_SELECTABLE", message: "该节点不可选" },
        },
      ],
    });
    const level = optionsForKey(scan, KEY);
    expect(level.selectable.map((option) => option.nodeId)).toEqual(["ok/1"]);
    expect(level.blocked.map((option) => option.nodeId)).toEqual(["bad/3"]);
  });
});
