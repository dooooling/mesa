// NCK 语义选型器（原内嵌在通用 Picker 里的 semantic 分支，PR #150 解耦后独立成件）。
// 断言逐条沿用搬迁前的口径：A21（语义页默认）+ A22（旧 variable 入口不存在）
// + 目录缺失 fail-closed（不退回旧原生字段流程）。
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { NckSemanticPicker } from "./NckSemanticPicker";
import type { NckPreviewFn } from "./NckPointFields";
import type { NckDefinitionLike } from "../nckPointModel";
import { selectVariableViaTree } from "../test/catalogTreePick";
import { CANDIDATE_AVAILABILITY } from "../test/availabilityFixture";

const definition: NckDefinitionLike = {
  variable_id: "syn.semantic",
  revision: 1,
  family: "828d",
  scope: "collect",
  targets: ["channel"],
  target_required: ["channel"],
  arguments: [],
  availability: CANDIDATE_AVAILABILITY,
};

const preview: NckPreviewFn = async (req) => ({
  point_key: `${String(req.parameters.variable_id)}@channel=i:1`,
  identity_mode: "derived",
  scope: "collect",
  variable_id: String(req.parameters.variable_id),
  definition_revision: Number(req.parameters.definition_revision),
  definition_digest: "d",
});

describe("NckSemanticPicker（A21/A22）", () => {
  it("semantic_pane_is_default_and_legacy_variable_entry_is_not_rendered", async () => {
    const onAddPoint = vi.fn((_point: unknown) => true);
    render(
      <NckSemanticPicker
        driverId="sinumerik-nck"
        definitions={[definition]}
        endpointFamily="828d"
        takenPointKeys={[]}
        onAddPoint={onAddPoint}
        preview={preview}
      />,
    );
    // A21：语义页是默认页；A22：旧 variable 手动入口整体不存在。
    expect(screen.getByTestId("nck-semantic-picker")).toBeTruthy();
    expect(screen.getByText("语义点位")).toBeTruthy();
    expect(screen.queryByText("手动")).toBeNull();
    expect(screen.queryByText("无可用资源")).toBeNull();

    selectVariableViaTree("syn.semantic");
    fireEvent.change(screen.getByLabelText("target-channel"), { target: { value: "1" } });
    await waitFor(() => expect(screen.getByTestId("nck-point-key")).toBeTruthy());
    fireEvent.click(screen.getByTestId("nck-add-point"));
    expect(onAddPoint).toHaveBeenCalledTimes(1);
    expect(onAddPoint.mock.calls[0][0]).toMatchObject({
      kind: "nck_variable",
      parameters: {
        variable_id: "syn.semantic",
        definition_revision: 1,
        target: { channel: 1 },
        arguments: {},
      },
    });
  });

  it("semantic_without_definitions_is_fail_closed_and_never_falls_back", () => {
    render(
      <NckSemanticPicker
        driverId="sinumerik-nck"
        definitions={null}
        endpointFamily="828d"
        onAddPoint={() => true}
        preview={preview}
      />,
    );
    expect(screen.getByTestId("nck-semantic-picker-no-definitions")).toBeTruthy();
    expect(screen.getByTestId("nck-semantic-picker-no-definitions-body")).toBeTruthy();
    // 不回退旧 manual：没有 variable 资源选择、没有旧“加入”按钮。
    expect(screen.queryByText("手动")).toBeNull();
    expect(screen.queryByText("资源")).toBeNull();
  });
});
