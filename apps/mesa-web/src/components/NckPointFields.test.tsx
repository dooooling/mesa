// PR-C 第二批 B：语义表单 + 权威预览状态机的组件级门禁。
//
// 竞态用 deferred promise 手工控制返回顺序（同仓先例：useConnectionSetup.test.ts
// 的「迟到响应整份丢弃」），断言口径不是「有没有 abort」，而是
// 「最终展示与最终提交的 Key 必须对应最后一次输入」。
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { NckPointFields, type NckPreviewFn } from "./NckPointFields";
import type { NckIdentityPreviewResponse } from "../nckCatalog";
import type { NckDefinitionLike } from "../nckPointModel";
import type { NckPointSelection } from "../deviceModel";
import type { SchemaDescriptor } from "../types";

function definition(overrides: Partial<NckDefinitionLike> = {}): NckDefinitionLike {
  return {
    variable_id: "syn.channel.status",
    revision: 1,
    family: "828d",
    scope: "collect",
    semantics_status: "verified",
    source: [{ kind: "manual" }],
    targets: ["channel"],
    target_required: ["channel"],
    arguments: [],
    identity_template: "syn.channel.status@channel={channel}",
    identity_keys: [{ path: "channel", required: true }],
    golden_ref: [{ entry: "nc_variable" }],
    ...overrides,
  };
}

function previewResponse(overrides: Partial<NckIdentityPreviewResponse> = {}): NckIdentityPreviewResponse {
  return {
    point_key: "syn.channel.status@channel=1",
    identity_mode: "derived",
    scope: "collect",
    variable_id: "syn.channel.status",
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

function renderFields(args: {
  definitions: NckDefinitionLike[];
  preview: NckPreviewFn;
  endpointFamily?: string | null;
  catalogFamily?: string | null;
  takenPointKeys?: string[];
  initialPoint?: NckPointSelection | null;
  onSubmit?: (point: NckPointSelection) => void;
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
      onSubmit={onSubmit}
      preview={args.preview}
    />,
  );
  return { onSubmit };
}

function choose(variableId: string) {
  fireEvent.change(screen.getByTestId("nck-variable-select"), {
    target: { value: variableId },
  });
}

describe("P0-1：未选择变量不显示虚假错误", () => {
  it("no_selection_shows_only_prompt_and_no_form_model_invalid", () => {
    const { preview } = installPreview();
    renderFields({ definitions: [definition()], preview });
    // 只提示「请选择可采集变量」，不出现校验错误 / FORM_MODEL_INVALID。
    expect(screen.getByTestId("nck-select-hint").textContent).toContain("请选择可采集变量");
    expect(screen.queryByTestId("nck-form-issues")).toBeNull();
    expect(screen.queryByTestId("nck-model-reject")).toBeNull();
    expect(screen.queryByText(/FORM_MODEL_INVALID/)).toBeNull();
    // 提交按钮保持禁用（不是报错，也不产生可提交路径）。
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(true);
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

  it("selected_definition_shows_input_requirements_and_validation_issues", () => {
    const { preview } = installPreview();
    renderFields({ definitions: [definition()], preview });
    choose("syn.channel.status");
    // 已选择：显示输入要求（Target 表单）与必填校验问题。
    expect(screen.getByText(/Target（由定义/)).toBeTruthy();
    expect(screen.getByTestId("nck-form-issues").textContent).toContain("MISSING_TARGET");
  });
});

describe("P1-1：变量选择可搜索（中文名称 / 变量 ID）", () => {
  function optionValues(): string[] {
    return Array.from(
      (screen.getByTestId("nck-variable-select") as HTMLSelectElement).querySelectorAll("option"),
    )
      .map((o) => o.value)
      .filter(Boolean);
  }

  const spindle = definition({
    variable_id: "syn.spindle.speed",
    label: { zh: "主轴转速", en: "Spindle speed" },
  });
  const channel = definition({
    variable_id: "syn.channel.status",
    label: { zh: "通道状态", en: "Channel status" },
  });

  it("filters_by_chinese_label_and_variable_id", () => {
    const { preview } = installPreview();
    renderFields({ definitions: [spindle, channel], preview });
    expect(optionValues()).toEqual(["syn.spindle.speed", "syn.channel.status"]);

    const search = screen.getByTestId("nck-variable-search");
    fireEvent.change(search, { target: { value: "主轴" } });
    expect(optionValues()).toEqual(["syn.spindle.speed"]);

    fireEvent.change(search, { target: { value: "channel.status" } });
    expect(optionValues()).toEqual(["syn.channel.status"]);

    fireEvent.change(search, { target: { value: "不存在的变量" } });
    expect(optionValues()).toEqual([]);
    expect(screen.getByTestId("nck-variable-search-count").textContent).toContain(
      "没有匹配的变量",
    );
  });

  it("keeps_selected_variable_visible_when_search_excludes_it", () => {
    const { preview } = installPreview();
    renderFields({ definitions: [spindle, channel], preview });
    choose("syn.spindle.speed");
    fireEvent.change(screen.getByTestId("nck-variable-search"), {
      target: { value: "通道" },
    });
    // 已选变量必须仍在下拉中：否则 select 会显示为空，用户会以为选择被清掉。
    expect(optionValues()).toEqual(["syn.spindle.speed", "syn.channel.status"]);
    expect((screen.getByTestId("nck-variable-select") as HTMLSelectElement).value).toBe(
      "syn.spindle.speed",
    );
  });
});

describe("门禁 4：Target/Arguments 动态生成，非法值拒绝", () => {
  it("dynamic_targets_and_arguments_follow_definition_without_hardcoding", () => {
    const compound = definition({
      variable_id: "syn.axis.position",
      targets: ["channel", "axis"],
      target_required: ["channel", "axis"],
      arguments: [
        { key: "index", label: "R 参数号", value_type: "integer", required: true, min: 0, max: 10 },
      ],
    });
    const drive = definition({
      variable_id: "syn.drive.param",
      targets: ["drive_object"],
      target_required: ["drive_object"],
      golden_ref: [{ entry: "nc_variable" }, { entry: "drive_object" }],
    });
    const { preview } = installPreview();
    renderFields({ definitions: [compound, drive], preview });

    choose("syn.axis.position");
    expect(screen.getByLabelText("target-channel")).toBeTruthy();
    expect(screen.getByLabelText("target-axis")).toBeTruthy();
    expect(screen.getByLabelText("argument-index")).toBeTruthy();

    // 切换变量：上一变量不兼容的 Target/Arguments 必须清空（A30/A31）。
    choose("syn.drive.param");
    expect(screen.queryByLabelText("target-axis")).toBeNull();
    expect(screen.queryByLabelText("argument-index")).toBeNull();
    const entry = screen.getByTestId("nck-target-drive_object-entry") as HTMLInputElement;
    // 目录未声明 entry 合法域 ⇒ 必须是自由输入 + datalist 示例，不能是闭集 select。
    expect(entry.tagName).toBe("INPUT");
    expect(screen.getByTestId("nck-entry-contract-gap")).toBeTruthy();
    const datalist = document.getElementById("nck-entry-examples-drive_object") as HTMLDataListElement;
    expect(Array.from(datalist.querySelectorAll("option")).map((o) => o.value)).toEqual([
      "nc_variable",
      "drive_object",
    ]);
  });

  it("golden_entries_are_examples_not_a_closed_domain", async () => {
    const { preview, pending } = installPreview();
    renderFields({
      definitions: [
        definition({
          variable_id: "syn.drive.param",
          targets: ["drive_object"],
          target_required: ["drive_object"],
          golden_ref: [{ entry: "nc_variable" }, { entry: "drive_object" }],
        }),
      ],
      preview,
    });
    choose("syn.drive.param");
    fireEvent.change(screen.getByLabelText("target-drive_object"), {
      target: { value: "26" },
    });
    // 不在 Golden 示例里的值不得在前端被当成非法（示例 ≠ 完整合法域）；
    // 它必须进入服务端 Preview，由服务端裁决。
    fireEvent.change(screen.getByLabelText("target-drive_object-entry"), {
      target: { value: "future_entry" },
    });
    expect(screen.queryByTestId("nck-form-issues")).toBeNull();
    await waitFor(() => expect(pending).toHaveLength(1));
    expect(pending[0].request.parameters).toEqual({
      variable_id: "syn.drive.param",
      definition_revision: 1,
      target: { drive_object: { number: 26, entry: "future_entry" } },
      arguments: {},
    });
    await act(async () => {
      pending[0].deferred.reject(new Error("UNSUPPORTED_TARGET_ENTRY"));
      await new Promise((r) => setTimeout(r, 10));
    });
    expect(screen.getByTestId("nck-preview-error").textContent).toContain(
      "UNSUPPORTED_TARGET_ENTRY",
    );
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(true);
  });

  it("invalid_values_are_rejected_before_preview_and_block_submit", () => {
    const { preview, pending } = installPreview();
    renderFields({
      definitions: [
        definition({
          arguments: [
            { key: "index", value_type: "integer", required: true, min: 0, max: 10 },
          ],
        }),
      ],
      preview,
    });
    choose("syn.channel.status");
    fireEvent.change(screen.getByLabelText("target-channel"), { target: { value: "abc" } });
    expect(screen.getByTestId("nck-form-issues").textContent).toContain("INVALID_TARGET");
    expect(pending).toHaveLength(0);

    fireEvent.change(screen.getByLabelText("target-channel"), { target: { value: "1" } });
    fireEvent.change(screen.getByLabelText("argument-index"), { target: { value: "11" } });
    expect(screen.getByTestId("nck-form-issues").textContent).toContain("INVALID_ARGUMENT");
    expect(pending).toHaveLength(0);
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(true);
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
          variable_id: "syn.channel.status",
          definition_revision: 1,
          target: { channel: 1 },
          arguments: {},
        },
      },
    });
    // 即使父层把 identity 定义塞进表单（绕过列表过滤/直接用初始点位注入），
    // 这里也必须拒绝；select 里不再提供 identity 选项，但 definition 仍能显示原因。
    const optionValues = Array.from(
      (screen.getByTestId("nck-variable-select") as HTMLSelectElement).querySelectorAll("option"),
    ).map((o) => o.value);
    expect(optionValues).not.toContain("syn.channel.status");
    expect(screen.getByTestId("nck-scope-reject").textContent).toContain("SCOPE_IDENTITY");
    expect(pending).toHaveLength(0);
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(true);
  });

  it("preview_scope_identity_is_rejected_even_when_directory_says_collect", async () => {
    const { preview, pending } = installPreview();
    renderFields({ definitions: [definition()], preview });
    choose("syn.channel.status");
    fireEvent.change(screen.getByLabelText("target-channel"), { target: { value: "1" } });
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
  it("family_mismatch_blocks_preview_and_submit", () => {
    const { preview, pending } = installPreview();
    renderFields({
      definitions: [definition({ family: "840d-sl" })],
      preview,
      endpointFamily: "828d",
    });
    choose("syn.channel.status");
    fireEvent.change(screen.getByLabelText("target-channel"), { target: { value: "1" } });
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
    choose("syn.channel.status");
    fireEvent.change(screen.getByLabelText("target-channel"), { target: { value: "1" } });
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
    choose("syn.channel.status");
    fireEvent.change(screen.getByLabelText("target-channel"), { target: { value: "1" } });
    await waitFor(() => expect(pending).toHaveLength(1));
    // 同一事件循环内改成 2：A 的 AbortController 被 abort，但桩**故意**仍会 resolve。
    fireEvent.change(screen.getByLabelText("target-channel"), { target: { value: "2" } });
    await waitFor(() => expect(pending).toHaveLength(2));
    // 预览请求与待提交 parameters 逐字同构（顶层只有 family/kind/parameters）。
    expect(pending[1].request.family).toBe("828d");
    expect(pending[1].request.kind).toBe("nck_variable");
    expect(pending[1].request.parameters).toEqual({
      variable_id: "syn.channel.status",
      definition_revision: 1,
      target: { channel: 2 },
      arguments: {},
    });

    // 先放行 B（最新输入）⇒ 显示 K_B。
    await act(async () => {
      pending[1].deferred.resolve(
        previewResponse({ point_key: "syn.channel.status@channel=2" }),
      );
      await Promise.resolve();
    });
    await waitFor(() =>
      expect(screen.getByTestId("nck-point-key").textContent).toBe(
        "syn.channel.status@channel=2",
      ),
    );

    // 再放行 A（迟到）⇒ 整份丢弃，K_A 不得在任何 UI 文本出现。
    await act(async () => {
      pending[0].deferred.resolve(
        previewResponse({ point_key: "syn.channel.status@channel=1" }),
      );
      await new Promise((r) => setTimeout(r, 10));
    });
    expect(screen.getByTestId("nck-point-key").textContent).toBe(
      "syn.channel.status@channel=2",
    );
    expect(screen.queryByText(/channel=1/)).toBeNull();

    fireEvent.click(screen.getByTestId("nck-add-point"));
    expect(onSubmit).toHaveBeenCalledTimes(1);
    const point = onSubmit.mock.calls[0][0] as NckPointSelection;
    expect(point.point_key).toBe("syn.channel.status@channel=2");
    expect(point.parameters).toEqual({
      variable_id: "syn.channel.status",
      definition_revision: 1,
      target: { channel: 2 },
      arguments: {},
    });
  });

  it("reversed_resolution_order_still_accepts_only_the_latest_sequence", async () => {
    const { preview, pending } = installPreview();
    renderFields({ definitions: [definition()], preview });
    choose("syn.channel.status");
    fireEvent.change(screen.getByLabelText("target-channel"), { target: { value: "1" } });
    await waitFor(() => expect(pending).toHaveLength(1));
    fireEvent.change(screen.getByLabelText("target-channel"), { target: { value: "2" } });
    await waitFor(() => expect(pending).toHaveLength(2));

    // 先放行 A（旧序号）：必须被丢弃。
    await act(async () => {
      pending[0].deferred.resolve(
        previewResponse({ point_key: "syn.channel.status@channel=1" }),
      );
      await new Promise((r) => setTimeout(r, 10));
    });
    expect(screen.queryByTestId("nck-point-key")).toBeNull();

    await act(async () => {
      pending[1].deferred.resolve(
        previewResponse({ point_key: "syn.channel.status@channel=2" }),
      );
      await Promise.resolve();
    });
    await waitFor(() =>
      expect(screen.getByTestId("nck-point-key").textContent).toBe(
        "syn.channel.status@channel=2",
      ),
    );
  });

  it("editing_target_invalidates_key_synchronously", async () => {
    const { preview, pending } = installPreview();
    renderFields({ definitions: [definition()], preview });
    choose("syn.channel.status");
    fireEvent.change(screen.getByLabelText("target-channel"), { target: { value: "1" } });
    await waitFor(() => expect(pending).toHaveLength(1));
    await act(async () => {
      pending[0].deferred.resolve(previewResponse());
      await Promise.resolve();
    });
    await waitFor(() =>
      expect(screen.getByTestId("nck-point-key").textContent).toBe(
        "syn.channel.status@channel=1",
      ),
    );

    // 不等任何响应：旧 Key 必须立刻从界面与提交模型中消失。
    fireEvent.change(screen.getByLabelText("target-channel"), { target: { value: "2" } });
    expect(screen.queryByTestId("nck-point-key")).toBeNull();
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(true);
  });
});

describe("门禁 6：Preview 失败不得回退旧 Key", () => {
  it("preview_failure_keeps_submit_disabled_and_never_reuses_previous_key", async () => {
    const { preview, pending } = installPreview();
    renderFields({ definitions: [definition()], preview });
    choose("syn.channel.status");
    fireEvent.change(screen.getByLabelText("target-channel"), { target: { value: "1" } });
    await waitFor(() => expect(pending).toHaveLength(1));
    await act(async () => {
      pending[0].deferred.resolve(previewResponse());
      await Promise.resolve();
    });
    await waitFor(() => expect(screen.getByTestId("nck-point-key")).toBeTruthy());

    fireEvent.change(screen.getByLabelText("target-channel"), { target: { value: "2" } });
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

describe("PR-C 第三批：高级原生模式入口", () => {
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
    fireEvent.click(screen.getByTestId("nck-native-add-point"));
    expect(onSubmit).toHaveBeenCalledTimes(1);
    expect((onSubmit.mock.calls[0][0] as NckPointSelection).kind).toBe("nck_native");
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
      takenPointKeys: ["syn.channel.status@channel=1"],
    });
    choose("syn.channel.status");
    fireEvent.change(screen.getByLabelText("target-channel"), { target: { value: "1" } });
    await waitFor(() => expect(pending).toHaveLength(1));
    await act(async () => {
      pending[0].deferred.resolve(previewResponse());
      await Promise.resolve();
    });
    await waitFor(() => expect(screen.getByTestId("nck-point-key")).toBeTruthy());
    // point_key 只读展示：没有可编辑的输入框（只有 code 文本）。
    expect(screen.queryByDisplayValue("syn.channel.status@channel=1")).toBeNull();
    expect(screen.getByTestId("nck-key-conflict")).toBeTruthy();
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(true);
  });
});
