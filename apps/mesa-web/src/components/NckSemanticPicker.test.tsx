// NCK 语义选型器（原内嵌在通用 Picker 里的 semantic 分支，PR #150 解耦后独立成件）。
// 断言逐条沿用搬迁前的口径：A21（语义页默认）+ A22（旧 variable 入口不存在）
// + 目录缺失 fail-closed（不退回旧原生字段流程）。
//
// 新增口径（联动选择收敛后）：调用方**没有 Endpoint** 时（创建向导就是这样），
// 语义点位无法探测实际对象 ⇒ 明确阻断并给出下一步，**不回退成手填参数表单**。
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
    // 联动选择出现，但调用方没给 Endpoint ⇒ 探测闸门关闭、原因可见。
    const select = screen.getByTestId("nck-object-select-channel") as HTMLSelectElement;
    expect(select.disabled).toBe(true);
    expect(screen.getByTestId("nck-cascade-gate").textContent).toContain("需先创建并连接端点");
    // 语义页没有任何手填参数表单（不回退）。
    expect(screen.queryByTestId(/^nck-target-/)).toBeNull();
    expect(screen.queryByTestId(/^nck-argument-/)).toBeNull();
    // 缺参数阻断 + 两条出路：先建设备到采集页配置，或切高级原生配置。
    const missing = screen.getByTestId("nck-cascade-missing");
    expect(missing.textContent).toContain("无法从设备探测获得");
    const guidance = screen.getByTestId("nck-cascade-guidance").textContent ?? "";
    expect(guidance).toContain("先完成设备创建与连接");
    expect(guidance).toContain("「采集」页配置点位");
    expect(guidance).toContain("「高级原生配置」");
    // 不可加入：按钮禁用，且程序化点击也不产生点位。
    const add = screen.getByTestId("nck-add-point") as HTMLButtonElement;
    expect(add.disabled).toBe(true);
    fireEvent.click(add);
    await waitFor(() => expect(onAddPoint).not.toHaveBeenCalled());
    expect(screen.queryByTestId("nck-point-key")).toBeNull();
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
