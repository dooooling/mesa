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
    const entry = screen.getByTestId("nck-target-drive_object-entry") as HTMLSelectElement;
    expect(Array.from(entry.querySelectorAll("option")).map((o) => o.value)).toEqual([
      "",
      "nc_variable",
      "drive_object",
    ]);
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
    });
    choose("syn.channel.status");
    // 即使父层把 identity 定义塞进表单（绕过列表过滤），这里也必须拒绝。
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
