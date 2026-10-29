// 语义表单（联动选择 + 权威预览状态机）的组件级门禁。
//
// 形态已收敛为**两个互斥入口**：
//   - 「联动选择」（默认）：选变量 → 逐级选实际对象 → 参数由驱动载荷自动填入；
//   - 「高级原生配置」：`NckNativeFields`，原样保留。
// 因此本文件**没有任何** Target/Arguments 手填用例——那组输入框已被移除，
// 取而代之的是"参数只能来自驱动载荷"的断言（见「门禁 4」与「缺参数阻断」）。
//
// 竞态用 deferred promise 手工控制返回顺序（同仓先例：useConnectionSetup.test.ts
// 的「迟到响应整份丢弃」），断言口径不是「有没有 abort」，而是
// 「最终展示与最终提交的 Key 必须对应最后一次输入」。
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { NckPointFields, type NckPreviewFn } from "./NckPointFields";
import {
  catalogLeafIds,
  expandAllCatalogGroups,
  openVariablePicker,
  selectVariableViaTree,
  selectedCatalogLeafIds,
} from "../test/catalogTreePick";
import { channelNode, emptyProbeResponse, pickObject, probeResponse } from "../test/objectCascadePick";
import type { NckIdentityPreviewResponse } from "../nckCatalog";
import type { NckDefinitionLike } from "../nckPointModel";
import type { NckPointSelection } from "../deviceModel";
import type { SchemaDescriptor } from "../types";
import { CANDIDATE_AVAILABILITY } from "../test/availabilityFixture";

/**
 * 变量 id 取自驱动夹具的 `identity.applies_to_variables`：
 * 候选归属由驱动声明决定，前端不按名字/前缀猜（见 objectCascade.nodeBelongsToVariable）。
 */
function definition(overrides: Partial<NckDefinitionLike> = {}): NckDefinitionLike {
  return {
    variable_id: "channel.feed.actual",
    revision: 1,
    family: "828d",
    scope: "collect",
    semantics_status: "verified",
    source: [{ kind: "manual" }],
    targets: ["channel"],
    target_required: ["channel"],
    arguments: [],
    availability: CANDIDATE_AVAILABILITY,
    identity_template: "channel.feed.actual@channel={channel}",
    identity_keys: [{ path: "channel", required: true }],
    golden_ref: [{ entry: "nc_variable" }],
    ...overrides,
  };
}

function previewResponse(
  overrides: Partial<NckIdentityPreviewResponse> = {},
): NckIdentityPreviewResponse {
  return {
    point_key: "channel.feed.actual@channel=1",
    identity_mode: "derived",
    scope: "collect",
    variable_id: "channel.feed.actual",
    definition_revision: 1,
    definition_digest: "digest",
    ...overrides,
  };
}

interface PendingPreview {
  request: {
    driverId: string;
    family: string;
    kind: string;
    parameters: Record<string, unknown>;
  };
  deferred: ReturnType<typeof deferred<NckIdentityPreviewResponse>>;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function installPreview() {
  const pending: PendingPreview[] = [];
  const preview: NckPreviewFn = (request) => {
    const d = deferred<NckIdentityPreviewResponse>();
    pending.push({ request, deferred: d });
    return d.promise;
  };
  return { preview, pending };
}

/** 探测桩：只服务驱动契约夹具导出的分支，其余分支如实返回 UNKNOWN_PARENT。 */
function installProbe(
  respond: (parent: string, cursor?: string) => unknown = probeResponse,
) {
  const parents: string[] = [];
  (globalThis as { fetch?: unknown }).fetch = vi.fn(
    async (_url: string, init?: RequestInit) => {
      const body = JSON.parse(String(init?.body ?? "{}")) as {
        parent?: string;
        cursor?: string;
      };
      const parent = body.parent ?? "";
      parents.push(parent);
      return respond(parent, body.cursor ?? "") as ReturnType<typeof probeResponse>;
    },
  );
  return parents;
}

/** 原生 12 字段 Schema（与向导/端点页同一份来源：Descriptor 的 variable 资源）。 */
const NATIVE_SCHEMA: SchemaDescriptor = {
  fields: [
    { key: "area", label: "Area", field_type: "enum", required: true, validation: { enum_options: ["C"] }, ui: {} },
    { key: "area_no", label: "Area No.", field_type: "integer", required: true, validation: { min: 0, max: 31 }, ui: {} },
    { key: "module", label: "Module", field_type: "integer", required: true, validation: { min: 0, max: 255 }, ui: {} },
    { key: "column", label: "Column", field_type: "integer", required: true, validation: { min: 0, max: 65535 }, ui: {} },
    { key: "line", label: "Line", field_type: "integer", required: true, validation: { min: 0, max: 65535 }, ui: {} },
  ],
};

function renderFields(args: {
  definitions: NckDefinitionLike[];
  preview: NckPreviewFn;
  endpointFamily?: string | null;
  catalogFamily?: string | null;
  takenPointKeys?: string[];
  initialPoint?: NckPointSelection | null;
  onSubmit?: (point: NckPointSelection) => void;
  endpointId?: string | null;
}) {
  const onSubmit = args.onSubmit ?? vi.fn();
  render(
    <NckPointFields
      driverId="sinumerik-nck"
      endpointFamily={args.endpointFamily === undefined ? "828d" : args.endpointFamily}
      catalogFamily={args.catalogFamily ?? null}
      definitions={args.definitions}
      takenPointKeys={args.takenPointKeys ?? []}
      initialPoint={args.initialPoint ?? null}
      endpointId={args.endpointId === undefined ? "ep-unit" : args.endpointId}
      // 端点已创建且连接可读 ⇒ 允许按需探测（与端点采集页的真实入参同形）。
      probeGate={
        args.endpointId === null
          ? undefined
          : { available: true }
      }
      nativeSchema={NATIVE_SCHEMA}
      onSubmit={onSubmit}
      preview={args.preview}
    />,
  );
  return { onSubmit };
}

/** 选中变量：唯一入口是分类树（平铺下拉框已按评审要求移除）。 */
function choose(variableId: string) {
  selectVariableViaTree(variableId);
}

beforeEach(() => {
  vi.restoreAllMocks();
  installProbe();
});

describe("P0-1：未选择变量不显示虚假错误", () => {
  it("no_selection_shows_only_prompt_and_no_form_model_invalid", () => {
    const { preview } = installPreview();
    renderFields({ definitions: [definition()], preview });
    // 只提示「请选择可采集变量」，不出现校验错误 / FORM_MODEL_INVALID。
    expect(screen.getByTestId("nck-select-hint").textContent).toContain("请选择可采集变量");
    expect(screen.queryByTestId("nck-form-issues")).toBeNull();
    expect(screen.queryByTestId("nck-model-reject")).toBeNull();
    expect(screen.queryByText(/FORM_MODEL_INVALID/)).toBeNull();
    // §4.1-4：未选变量时只保留**一个选择入口**——不出现空身份、错误框与技术细节；
    // 添加按钮**在场但禁用**（首屏就能看到主操作在哪，不需要先猜流程）。
    const add = screen.getByTestId("nck-add-point") as HTMLButtonElement;
    expect(add.disabled).toBe(true);
    expect(screen.queryByTestId("nck-point-key-empty")).toBeNull();
    // 唯一选择入口 = 默认收起的树形下拉（展开前没有搜索框/叶子，不再有平铺下拉框）。
    expect(screen.getByTestId("nck-variable-picker-trigger")).toBeTruthy();
    expect(screen.getByTestId("catalog-tree")).toBeTruthy();
    expect(screen.queryByTestId("catalog-tree-panel")).toBeNull();
    expect(screen.queryByTestId("nck-variable-search")).toBeNull();
    expect(screen.queryByTestId("nck-variable-select")).toBeNull();
    expect(screen.queryByTestId("nck-tech-details")).toBeNull();
  });

  it("selected_variable_without_definition_still_reports_real_error", () => {
    renderFields({
      definitions: [],
      preview: installPreview().preview,
      initialPoint: {
        kind: "nck_variable",
        point_key: "",
        parameters: {
          variable_id: "syn.missing",
          definition_revision: 1,
          target: { channel: 1 },
          arguments: {},
        },
      },
    });
    // 真正的定义异常不得被“未选择”的宽松分支吞掉。
    expect(screen.getByTestId("nck-definition-missing").textContent).toContain("定义缺失");
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(true);
  });

  it("selected_definition_shows_cascade_levels_and_blocks_until_object_is_picked", () => {
    const { preview } = installPreview();
    renderFields({ definitions: [definition()], preview });
    choose("channel.feed.actual");
    // 已选择：出现**级联下拉**（按变量需要的参数逐级出现），没有任何手填输入框。
    expect(screen.getByTestId("nck-object-level-channel")).toBeTruthy();
    expect(screen.getByTestId("nck-object-select-channel")).toBeTruthy();
    expect(screen.queryByTestId(/^nck-target-/)).toBeNull();
    expect(screen.queryByTestId(/^nck-argument-/)).toBeNull();
    // 未选对象前：提示"展开下拉选择"，添加被阻断，且不重复报"定义约束未通过"
    // （那只是流程没走完，不是输入错误）。
    expect(screen.getByTestId("nck-cascade-hint").textContent).toContain("请展开");
    expect(screen.queryByTestId("nck-form-issues")).toBeNull();
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(true);
  });
});

describe("P1-1：变量选择可搜索（中文名称 / 变量 ID）", () => {
  // 搜索继续过滤**树中的条目**：断言口径从「下拉框的 option 集合」换成
  // 「树里可见的叶子集合」，两者都表示"当前可选的变量集合"，语义等价。
  function visibleLeaves(): string[] {
    return catalogLeafIds();
  }

  const spindle = definition({
    variable_id: "spindle.speed.actual",
    label: { zh: "主轴转速", en: "Spindle speed" },
    targets: ["channel", "spindle"],
    target_required: ["channel", "spindle"],
    identity_keys: [
      { path: "channel", required: true },
      { path: "spindle", required: true },
    ],
    // 与 channel 定义区分开的必填参数：用于证明「搜索把条目过滤掉」时
    // 已选变量本身没有被清空（表单仍是它的表单）。
    arguments: [
      { key: "index", label: "主轴号", value_type: "integer", required: true, min: 1, max: 10 },
    ],
  });
  const channel = definition({
    variable_id: "channel.feed.actual",
    label: { zh: "通道状态", en: "Channel status" },
  });

  it("filters_by_chinese_label_and_variable_id", () => {
    const { preview } = installPreview();
    renderFields({ definitions: [spindle, channel], preview });
    // 下拉默认收起：先显式打开并展开分组，才看得到全部叶子。
    openVariablePicker();
    expandAllCatalogGroups();
    expect(visibleLeaves()).toEqual(["spindle.speed.actual", "channel.feed.actual"]);

    const search = screen.getByTestId("nck-variable-search");
    fireEvent.change(search, { target: { value: "主轴" } });
    expect(visibleLeaves()).toEqual(["spindle.speed.actual"]);

    fireEvent.change(search, { target: { value: "feed.actual" } });
    expect(visibleLeaves()).toEqual(["channel.feed.actual"]);

    fireEvent.change(search, { target: { value: "不存在的变量" } });
    expect(visibleLeaves()).toEqual([]);
    expect(screen.getByTestId("nck-variable-search-count").textContent).toContain(
      "没有匹配的变量",
    );
  });

  it("keeps_selected_variable_visible_when_search_excludes_it", () => {
    const { preview } = installPreview();
    renderFields({ definitions: [spindle, channel], preview });
    choose("spindle.speed.actual");
    // 选中态由树自身标记（叶子 data-selected）；选择后下拉自动收起。
    openVariablePicker();
    expandAllCatalogGroups();
    expect(selectedCatalogLeafIds()).toEqual(["spindle.speed.actual"]);

    fireEvent.change(screen.getByTestId("nck-variable-search"), {
      target: { value: "通道" },
    });
    // 搜索只过滤树里的条目（spindle 不再出现在树上）……
    expect(visibleLeaves()).toEqual(["channel.feed.actual"]);
    // ……但**不得清空已选变量**：表单仍是 spindle 的定义表单（它的必填参数
    // channel / spindle / index 三级都在），且不会误报「定义缺失」。
    expect(screen.getByTestId("nck-object-level-channel")).toBeTruthy();
    expect(screen.getByTestId("nck-object-level-spindle")).toBeTruthy();
    expect(screen.getByTestId("nck-object-level-index")).toBeTruthy();
    expect(screen.queryByTestId("nck-definition-missing")).toBeNull();
    expect(screen.getByTestId("nck-variable-search-count").textContent).toContain(
      "匹配 1 个变量",
    );
  });
});

describe("门禁 4：参数只来自驱动载荷（不再有手填 Target/Arguments）", () => {
  it("cascade_levels_follow_definition_and_switch_clears_previous_levels", async () => {
    const compound = definition({
      variable_id: "spindle.speed.actual",
      targets: ["channel", "spindle"],
      target_required: ["channel", "spindle"],
      identity_keys: [
        { path: "channel", required: true },
        { path: "spindle", required: true },
      ],
      arguments: [
        { key: "index", label: "R 参数号", value_type: "integer", required: true, min: 0, max: 10 },
      ],
    });
    const drive = definition({
      variable_id: "channel.r_parameter",
      targets: ["drive_object"],
      target_required: ["drive_object"],
      identity_keys: [{ path: "drive_object", required: true }],
      golden_ref: [{ entry: "nc_variable" }, { entry: "drive_object" }],
    });
    const { preview } = installPreview();
    renderFields({ definitions: [compound, drive], preview });

    choose("spindle.speed.actual");
    // 三级都来自定义声明（顺序与定义一致），且全是下拉而非输入框。
    expect(screen.getByTestId("nck-object-select-channel")).toBeTruthy();
    expect(screen.getByTestId("nck-object-select-spindle")).toBeTruthy();
    expect(screen.getByTestId("nck-object-select-index")).toBeTruthy();
    expect(screen.queryByTestId(/^nck-target-/)).toBeNull();

    // 切换变量：上一变量的级联级必须整体消失（不残留、不串用旧选择）。
    choose("channel.r_parameter");
    expect(screen.queryByTestId("nck-object-level-spindle")).toBeNull();
    expect(screen.queryByTestId("nck-object-level-index")).toBeNull();
    expect(screen.getByTestId("nck-object-level-drive_object")).toBeTruthy();
    // entry 也没有输入框：它只能随驱动载荷一起给出（技术详情里说明契约缺口）。
    expect(screen.queryByTestId("nck-target-drive_object-entry")).toBeNull();
    expect(screen.getByTestId("nck-tech-details")).toBeTruthy();
    expect(screen.queryByTestId("nck-entry-contract-gap")).toBeNull();
    await act(async () => {
      fireEvent.click(screen.getByText(/技术详情/));
    });
    expect(screen.getByTestId("nck-entry-contract-gap").textContent).toContain(
      "entry 只能取自驱动载荷",
    );
  });

  it("drive_object_entry_is_never_defaulted_and_missing_params_block_with_reason", async () => {
    // 探测**成功**（所有分支都返回空页）⇒ 口径是"设备没有返回能提供该参数的对象"。
    installProbe(() => emptyProbeResponse());
    const { preview, pending } = installPreview();
    renderFields({
      definitions: [
        definition({
          variable_id: "channel.r_parameter",
          targets: ["drive_object"],
          target_required: ["drive_object"],
          identity_keys: [{ path: "drive_object", required: true }],
          golden_ref: [{ entry: "nc_variable" }, { entry: "drive_object" }],
        }),
      ],
      preview,
    });
    choose("channel.r_parameter");
    // 展开下拉 ⇒ 探测；驱动契约里没有能提供 drive_object 的对象。
    fireEvent.focus(screen.getByTestId("nck-object-select-drive_object"));
    const missing = await screen.findByTestId("nck-cascade-missing");
    // 明确提示"缺参数且无法从设备探测获得"，并给出下一步（不是让人手填）。
    expect(missing.textContent).toContain("缺少参数 drive_object");
    expect(missing.textContent).toContain("无法从设备探测获得");
    expect(missing.getAttribute("data-reason")).toContain("no_candidates");
    expect(screen.getByTestId("nck-cascade-guidance").textContent).toContain(
      "「高级原生配置」",
    );
    // 不填默认值：没有预览请求、没有 Key、不可添加。
    expect(pending).toHaveLength(0);
    expect(screen.queryByTestId("nck-point-key")).toBeNull();
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(true);
  });

  it("probe_failure_is_distinguishable_from_successful_but_empty_probe", async () => {
    // 所有分支都不可达：口径必须是"探测失败（不代表设备上没有）"，而不是"设备没有"。
    installProbe(() => ({
      ok: false,
      status: 400,
      json: async () => ({
        error: { code: "CONNECTION_LOST", message: "驱动进程已退出" },
      }),
    }));
    const { preview } = installPreview();
    renderFields({ definitions: [definition()], preview });
    choose("channel.feed.actual");
    fireEvent.focus(screen.getByTestId("nck-object-select-channel"));

    // 整次探测失败必须可见（带稳定码），不得渲染成空下拉。
    const error = await screen.findByTestId("nck-cascade-error");
    expect(error.getAttribute("data-code")).toBe("PROBE_REQUEST_FAILED");
    expect(error.textContent).toContain("不会用空下拉冒充结果");
    // 缺参数原因区分"分支不可达"与"设备确实没有"。
    const missing = await screen.findByTestId("nck-cascade-missing");
    expect(missing.getAttribute("data-reason")).toContain("probe_failed");
    expect(missing.textContent).toContain("探测失败，不代表设备上没有该对象");
    expect(missing.textContent).not.toContain("探测成功");
    // 下拉本身也不是"静默空"：占位文案写明探测失败。
    const select = screen.getByTestId("nck-object-select-channel") as HTMLSelectElement;
    expect(select.querySelector("option")?.textContent).toContain("探测失败");
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(true);
  });

  it("auto_filled_values_come_from_the_driver_payload_not_from_typing", async () => {
    const { preview, pending } = installPreview();
    renderFields({ definitions: [definition()], preview });
    choose("channel.feed.actual");
    // 没有任何可键入的输入框（唯一的输入是变量搜索框）。
    expect(screen.queryByTestId(/^nck-target-/)).toBeNull();
    expect(screen.queryByTestId(/^nck-argument-/)).toBeNull();

    await pickObject("channel", "通道2");
    // 参数由载荷自动填入：草稿值 = 驱动载荷里的 channel=2。
    expect(screen.getByTestId("nck-object-selected-channel").getAttribute("data-value")).toBe("2");
    expect(screen.getByTestId("nck-fill-notice").textContent).toContain(
      "参数已按所选实际对象填入",
    );
    await waitFor(() => expect(pending).toHaveLength(1));
    expect(pending[0].request.parameters).toEqual({
      variable_id: "channel.feed.actual",
      definition_revision: 1,
      target: { channel: 2 },
      arguments: {},
    });
    // 载荷里的值同时是选中节点自带的那个值（不是前端换算出来的）。
    expect(channelNode(2).selection_payload_json).toContain('"channel":2');
  });
});

describe("门禁 3：identity 不可新增（Preview 成功也不放行）", () => {
  it("forced_identity_selection_never_calls_preview_and_blocks_add", () => {
    const { preview, pending } = installPreview();
    renderFields({
      definitions: [definition({ scope: "identity" })],
      preview,
      initialPoint: {
        kind: "nck_variable",
        point_key: "",
        parameters: {
          variable_id: "channel.feed.actual",
          definition_revision: 1,
          target: { channel: 1 },
          arguments: {},
        },
      },
    });
    // 即使父层把 identity 定义塞进表单（绕过列表过滤/直接用初始点位注入），
    // 这里也必须拒绝；树上不再提供 identity 叶子（可选项集合里没有它），
    // 但 definition 仍能显示原因。
    openVariablePicker();
    expandAllCatalogGroups();
    expect(catalogLeafIds()).not.toContain("channel.feed.actual");
    expect(screen.getByTestId("nck-scope-reject").textContent).toContain("SCOPE_IDENTITY");
    expect(pending).toHaveLength(0);
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(true);
  });

  it("preview_scope_identity_is_rejected_even_when_directory_says_collect", async () => {
    const { preview, pending } = installPreview();
    renderFields({ definitions: [definition()], preview });
    choose("channel.feed.actual");
    await pickObject("channel", "通道1");
    await waitFor(() => expect(pending).toHaveLength(1));
    await act(async () => {
      pending[0].deferred.resolve(previewResponse({ scope: "identity" }));
      await Promise.resolve();
    });
    await waitFor(() =>
      expect(screen.getByTestId("nck-preview-error").textContent).toContain(
        "PREVIEW_SCOPE_IDENTITY",
      ),
    );
    expect(screen.queryByTestId("nck-point-key")).toBeNull();
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(true);
  });
});

describe("门禁 2：family 交叉校验决定是否发预览", () => {
  it("family_mismatch_blocks_preview_and_submit", async () => {
    const { preview, pending } = installPreview();
    renderFields({
      definitions: [definition({ family: "840d-sl" })],
      preview,
      endpointFamily: "828d",
    });
    choose("channel.feed.actual");
    await pickObject("channel", "通道1");
    expect(screen.getByTestId("nck-family-reject").textContent).toContain("FAMILY_MISMATCH");
    expect(pending).toHaveLength(0);
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(true);
  });

  it("catalog_top_level_null_does_not_block_when_endpoint_matches_definition", async () => {
    const { preview, pending } = installPreview();
    renderFields({
      definitions: [definition()],
      preview,
      endpointFamily: "828d",
      catalogFamily: null,
    });
    choose("channel.feed.actual");
    await pickObject("channel", "通道1");
    await waitFor(() => expect(pending).toHaveLength(1));
    await act(async () => {
      pending[0].deferred.resolve(previewResponse());
      await Promise.resolve();
    });
    await waitFor(() => expect(screen.getByTestId("nck-point-key")).toBeTruthy());
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(false);
  });
});

describe("门禁 5：Preview 竞态（乱序返回只采纳最后一次输入）", () => {
  it("stale_preview_response_never_overwrites_latest_identity", async () => {
    const { preview, pending } = installPreview();
    const onSubmit = vi.fn();
    renderFields({ definitions: [definition()], preview, onSubmit });
    choose("channel.feed.actual");
    await pickObject("channel", "通道1");
    await waitFor(() => expect(pending).toHaveLength(1));
    // 重新选择通道 2：A 的 AbortController 被 abort，但桩**故意**仍会 resolve。
    fireEvent.click(screen.getByTestId("nck-object-reselect-channel"));
    await pickObject("channel", "通道2");
    await waitFor(() => expect(pending).toHaveLength(2));
    // 预览请求与待提交 parameters 逐字同构（顶层只有 family/kind/parameters）。
    expect(pending[1].request.family).toBe("828d");
    expect(pending[1].request.kind).toBe("nck_variable");
    expect(pending[1].request.parameters).toEqual({
      variable_id: "channel.feed.actual",
      definition_revision: 1,
      target: { channel: 2 },
      arguments: {},
    });

    // 先放行 B（最新输入）⇒ 显示 K_B。
    await act(async () => {
      pending[1].deferred.resolve(
        previewResponse({ point_key: "channel.feed.actual@channel=2" }),
      );
      await Promise.resolve();
    });
    await waitFor(() =>
      expect(screen.getByTestId("nck-point-key").textContent).toBe(
        "channel.feed.actual@channel=2",
      ),
    );

    // 再放行 A（迟到）⇒ 整份丢弃，K_A 不得在任何 UI 文本出现。
    await act(async () => {
      pending[0].deferred.resolve(
        previewResponse({ point_key: "channel.feed.actual@channel=1" }),
      );
      await new Promise((r) => setTimeout(r, 10));
    });
    expect(screen.getByTestId("nck-point-key").textContent).toBe(
      "channel.feed.actual@channel=2",
    );
    expect(screen.queryByText(/channel=1/)).toBeNull();

    fireEvent.click(screen.getByTestId("nck-add-point"));
    expect(onSubmit).toHaveBeenCalledTimes(1);
    const point = onSubmit.mock.calls[0][0] as NckPointSelection;
    expect(point.point_key).toBe("channel.feed.actual@channel=2");
    expect(point.parameters).toEqual({
      variable_id: "channel.feed.actual",
      definition_revision: 1,
      target: { channel: 2 },
      arguments: {},
    });
  });

  it("reversed_resolution_order_still_accepts_only_the_latest_sequence", async () => {
    const { preview, pending } = installPreview();
    renderFields({ definitions: [definition()], preview });
    choose("channel.feed.actual");
    await pickObject("channel", "通道1");
    await waitFor(() => expect(pending).toHaveLength(1));
    fireEvent.click(screen.getByTestId("nck-object-reselect-channel"));
    await pickObject("channel", "通道2");
    await waitFor(() => expect(pending).toHaveLength(2));

    // 先放行 A（旧序号）：必须被丢弃。
    await act(async () => {
      pending[0].deferred.resolve(
        previewResponse({ point_key: "channel.feed.actual@channel=1" }),
      );
      await new Promise((r) => setTimeout(r, 10));
    });
    expect(screen.queryByTestId("nck-point-key")).toBeNull();

    await act(async () => {
      pending[1].deferred.resolve(
        previewResponse({ point_key: "channel.feed.actual@channel=2" }),
      );
      await Promise.resolve();
    });
    await waitFor(() =>
      expect(screen.getByTestId("nck-point-key").textContent).toBe(
        "channel.feed.actual@channel=2",
      ),
    );
  });

  it("changing_the_selected_object_invalidates_key_synchronously", async () => {
    const { preview, pending } = installPreview();
    renderFields({ definitions: [definition()], preview });
    choose("channel.feed.actual");
    await pickObject("channel", "通道1");
    await waitFor(() => expect(pending).toHaveLength(1));
    await act(async () => {
      pending[0].deferred.resolve(previewResponse());
      await Promise.resolve();
    });
    await waitFor(() =>
      expect(screen.getByTestId("nck-point-key").textContent).toBe(
        "channel.feed.actual@channel=1",
      ),
    );

    // 不等任何响应：点「重新选择」后旧 Key 必须立刻从界面与提交模型中消失。
    fireEvent.click(screen.getByTestId("nck-object-reselect-channel"));
    expect(screen.queryByTestId("nck-point-key")).toBeNull();
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(true);
  });
});

describe("门禁 6：Preview 失败不得回退旧 Key", () => {
  it("preview_failure_keeps_submit_disabled_and_never_reuses_previous_key", async () => {
    const { preview, pending } = installPreview();
    renderFields({ definitions: [definition()], preview });
    choose("channel.feed.actual");
    await pickObject("channel", "通道1");
    await waitFor(() => expect(pending).toHaveLength(1));
    await act(async () => {
      pending[0].deferred.resolve(previewResponse());
      await Promise.resolve();
    });
    await waitFor(() => expect(screen.getByTestId("nck-point-key")).toBeTruthy());

    fireEvent.click(screen.getByTestId("nck-object-reselect-channel"));
    await pickObject("channel", "通道2");
    await waitFor(() => expect(pending).toHaveLength(2));
    await act(async () => {
      pending[1].deferred.reject(new Error("driver unavailable"));
      await new Promise((r) => setTimeout(r, 10));
    });
    expect(screen.queryByTestId("nck-point-key")).toBeNull();
    expect(screen.getByTestId("nck-preview-error").textContent).toContain("driver unavailable");
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(true);
  });
});

describe("PR-C 第三批：高级原生模式入口（两个入口互斥）", () => {
  const nativeSchema: SchemaDescriptor = {
    fields: [
      { key: "area", label: "Area", field_type: "enum", required: true, validation: { enum_options: ["C"] }, ui: {} },
      { key: "area_no", label: "Area No.", field_type: "integer", required: true, validation: { min: 0, max: 31 }, ui: {} },
      { key: "module", label: "Module", field_type: "integer", required: true, validation: { min: 0, max: 255 }, ui: {} },
      { key: "column", label: "Column", field_type: "integer", required: true, validation: { min: 0, max: 65535 }, ui: {} },
      { key: "line", label: "Line", field_type: "integer", required: true, validation: { min: 0, max: 65535 }, ui: {} },
    ],
  };
  const nativePoint: NckPointSelection = {
    kind: "nck_native",
    point_key: "native.C.1.21.1.396",
    parameters: { area: "C", area_no: 1, module: 21, column: 1, line: 396 },
  };

  it("native_initial_point_opens_native_editor_and_submits_nck_native", () => {
    const onSubmit = vi.fn();
    render(
      <NckPointFields
        driverId="sinumerik-nck"
        endpointFamily="828d"
        definitions={[]}
        takenPointKeys={[]}
        initialPoint={nativePoint}
        nativeSchema={nativeSchema}
        onSubmit={onSubmit}
      />,
    );
    expect(screen.getByTestId("nck-editor-mode")).toBeTruthy();
    expect(screen.getByTestId("nck-native-fields")).toBeTruthy();
    expect(screen.getByTestId("nck-native-key").textContent).toBe("native.C.1.21.1.396");
    // 原生模式下**不渲染**联动表单（任何时刻只有一套表单在场）。
    expect(screen.queryByTestId("nck-object-cascade")).toBeNull();
    expect(screen.queryByTestId("catalog-tree")).toBeNull();
    fireEvent.click(screen.getByTestId("nck-native-add-point"));
    expect(onSubmit).toHaveBeenCalledTimes(1);
    expect((onSubmit.mock.calls[0][0] as NckPointSelection).kind).toBe("nck_native");
  });

  it("switching_modes_shows_exactly_one_form_at_a_time", () => {
    const { preview } = installPreview();
    renderFields({ definitions: [definition()], preview });
    choose("channel.feed.actual");
    expect(screen.getByTestId("nck-object-cascade")).toBeTruthy();
    expect(screen.queryByTestId("nck-native-fields")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "高级原生配置" }));
    expect(screen.queryByTestId("nck-object-cascade")).toBeNull();
    expect(screen.queryByTestId("catalog-tree")).toBeNull();
    expect(screen.getByTestId("nck-editor-mode").getAttribute("data-native-collapsed")).toBe(
      "false",
    );

    fireEvent.click(screen.getByRole("button", { name: "联动选择" }));
    expect(screen.getByTestId("nck-object-cascade")).toBeTruthy();
    expect(screen.queryByTestId("nck-native-fields")).toBeNull();
    expect(screen.getByTestId("nck-editor-mode").getAttribute("data-native-collapsed")).toBe(
      "true",
    );
  });

  it("native_without_schema_is_fail_closed", () => {
    render(
      <NckPointFields
        driverId="sinumerik-nck"
        endpointFamily="828d"
        definitions={[]}
        takenPointKeys={[]}
        initialPoint={nativePoint}
        nativeSchema={null}
        onSubmit={() => {}}
      />,
    );
    expect(screen.getByTestId("nck-native-schema-missing")).toBeTruthy();
    expect(screen.queryByTestId("nck-native-add-point")).toBeNull();
  });
});

describe("提交模型：point_key 只读、冲突拒绝、恰 3 字段", () => {
  it("readonly_key_and_conflict_blocks_add", async () => {
    const { preview, pending } = installPreview();
    renderFields({
      definitions: [definition()],
      preview,
      takenPointKeys: ["channel.feed.actual@channel=1"],
    });
    choose("channel.feed.actual");
    await pickObject("channel", "通道1");
    await waitFor(() => expect(pending).toHaveLength(1));
    await act(async () => {
      pending[0].deferred.resolve(previewResponse());
      await Promise.resolve();
    });
    await waitFor(() => expect(screen.getByTestId("nck-point-key")).toBeTruthy());
    // point_key 只读展示：没有可编辑的输入框（只有 code 文本）。
    expect(screen.queryByDisplayValue("channel.feed.actual@channel=1")).toBeNull();
    expect(screen.getByTestId("nck-key-conflict")).toBeTruthy();
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(true);
  });
});

describe("元数据分层：主视觉只留单位/可用性，其余进技术详情", () => {
  it("tech_details_are_collapsed_by_default_and_expand_on_demand", () => {
    const { preview } = installPreview();
    renderFields({ definitions: [definition()], preview });
    choose("channel.feed.actual");

    // 主视觉：只留名称/可用状态；单位与 ID 收在默认折叠的「变量详情」里。
    expect(screen.getByTestId("nck-availability-tag")).toBeTruthy();
    const variableDetails = screen.getByTestId("nck-variable-details") as HTMLDetailsElement;
    expect(variableDetails.open).toBe(false);
    expect(variableDetails.textContent).toContain("变量详情");
    expect((variableDetails.querySelector('[data-testid="nck-unit"]') as HTMLElement).textContent)
      .toBe("单位未核实");
    // 排查用信息默认**不渲染**（折叠，不占主视觉）。
    expect(screen.queryByTestId("nck-manual-location")).toBeNull();
    expect(screen.queryByTestId("nck-index-rule")).toBeNull();
    expect(screen.queryByTestId("nck-availability-static-note")).toBeNull();
    expect(screen.queryByTestId("nck-object-dynamic-state")).toBeNull();
    expect(screen.getByTestId("nck-tech-details")).toBeTruthy();

    fireEvent.click(screen.getByText(/技术详情/));
    expect(screen.getByTestId("nck-index-rule")).toBeTruthy();
    expect(screen.getByTestId("nck-availability-static-note").textContent).toContain(
      "与当前机床是否存在该对象无关",
    );
    expect(screen.getByTestId("nck-object-dynamic-state").getAttribute("data-object-state")).toBe(
      "unconfirmed",
    );
  });
});

describe("缺参数阻断：端点不可探测时明确提示并给出下一步", () => {
  it("unavailable_endpoint_blocks_with_actionable_guidance_and_zero_requests", () => {
    const { preview, pending } = installPreview();
    const fetchStub = vi.fn();
    (globalThis as { fetch?: unknown }).fetch = fetchStub;
    renderFields({ definitions: [definition()], preview, endpointId: null });
    choose("channel.feed.actual");

    // 端点不可用：下拉禁用 + 原因可见，且**一个请求都不发**。
    const select = screen.getByTestId("nck-object-select-channel") as HTMLSelectElement;
    expect(select.disabled).toBe(true);
    expect(screen.getByTestId("nck-cascade-gate").textContent).toContain("需先创建并连接端点");
    expect(fetchStub).not.toHaveBeenCalled();
    // 阻断文案必须给出两条出路（先建端点 → 采集页；或切高级原生配置）。
    const missing = screen.getByTestId("nck-cascade-missing");
    expect(missing.getAttribute("data-reason")).toContain("probe_unavailable");
    expect(missing.textContent).toContain("无法从设备探测获得");
    const guidance = screen.getByTestId("nck-cascade-guidance").textContent ?? "";
    expect(guidance).toContain("先完成设备创建与连接");
    expect(guidance).toContain("「采集」页配置点位");
    expect(guidance).toContain("「高级原生配置」");
    expect(pending).toHaveLength(0);
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(true);
  });
});
