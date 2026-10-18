// PR-C 第三批：nck_native 表单（A24–A27）。
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { NckNativeFields } from "./NckNativeFields";
import type { SchemaDescriptor } from "../types";
import type { NckPointSelection } from "../deviceModel";

const schema: SchemaDescriptor = {
  fields: [
    { key: "area", label: "Area", field_type: "enum", required: true, validation: { enum_options: ["N", "C"] }, ui: {} },
    { key: "area_no", label: "Area No.", field_type: "integer", required: true, validation: { min: 0, max: 31 }, ui: {} },
    { key: "module", label: "Module", field_type: "integer", required: true, validation: { min: 0, max: 255 }, ui: {} },
    { key: "column", label: "Column", field_type: "integer", required: true, validation: { min: 0, max: 65535 }, ui: {} },
    { key: "line", label: "Line", field_type: "integer", required: true, validation: { min: 0, max: 65535 }, ui: {} },
    { key: "count", label: "Count", field_type: "integer", required: true, default: 1, validation: { min: 1, max: 255 }, ui: {} },
    { key: "unit_mode", label: "Unit mode", field_type: "enum", required: true, default: "current", validation: { enum_options: ["current"] }, ui: {} },
    { key: "decode", label: "Decode", field_type: "enum", required: true, validation: { enum_options: ["U16_LE"] }, ui: {} },
    { key: "transport_size", label: "Transport", field_type: "enum", required: true, validation: { enum_options: ["0x05"] }, ui: {} },
  ],
};

function nativePoint(overrides: Partial<Record<string, unknown>> = {}): NckPointSelection {
  return {
    kind: "nck_native",
    point_key: "native.C.1.21.1.396",
    parameters: {
      area: "C",
      area_no: 1,
      module: 21,
      column: 1,
      line: 396,
      count: 1,
      unit_mode: "current",
      decode: "U16_LE",
      transport_size: "0x05",
      ...overrides,
    },
  };
}

describe("A24–A27：native 表单身份模式与提交", () => {
  it("derived_is_default_and_key_uses_frozen_rule", () => {
    const onSubmit = vi.fn();
    render(
      <NckNativeFields schema={schema} takenPointKeys={[]} initialPoint={nativePoint()} onSubmit={onSubmit} />,
    );
    expect(screen.getByTestId("nck-native-key").textContent).toBe("native.C.1.21.1.396");
    const btn = screen.getByTestId("nck-native-add-point") as HTMLButtonElement;
    expect(btn.disabled).toBe(false);
    fireEvent.click(btn);
    expect(onSubmit).toHaveBeenCalledTimes(1);
    const point = onSubmit.mock.calls[0][0] as NckPointSelection;
    expect(point.kind).toBe("nck_native");
    expect(point.point_key).toBe("native.C.1.21.1.396");
    expect(point.parameters).not.toHaveProperty("identity_mode");
  });

  it("user_mode_requires_explicit_confirmation_and_preserves_key (A24/A25)", () => {
    const onSubmit = vi.fn();
    render(
      <NckNativeFields schema={schema} takenPointKeys={[]} initialPoint={nativePoint()} onSubmit={onSubmit} />,
    );
    const radios = screen.getAllByRole("radio") as HTMLInputElement[];
    fireEvent.click(radios[1]); // user
    const input = screen.getByTestId("nck-native-user-key") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "native.C.1.21.1.397~2" } });
    const btn = screen.getByTestId("nck-native-add-point") as HTMLButtonElement;
    // 未勾选二次确认 ⇒ 不可提交。
    expect(btn.disabled).toBe(true);
    fireEvent.click(screen.getByTestId("nck-native-user-confirm"));
    expect(btn.disabled).toBe(false);
    fireEvent.click(btn);
    const point = onSubmit.mock.calls[0][0] as NckPointSelection;
    // 身份保持原值：不自动改名、不自动追加后缀。
    expect(point.point_key).toBe("native.C.1.21.1.397~2");
    expect(point.parameters.identity_mode).toBe("user");
  });

  it("invalid_user_namespace_and_conflict_block_submit (A26/A25)", () => {
    const onSubmit = vi.fn();
    render(
      <NckNativeFields
        schema={schema}
        takenPointKeys={["native.C.1.21.1.397"]}
        initialPoint={nativePoint()}
        onSubmit={onSubmit}
      />,
    );
    const radios = screen.getAllByRole("radio") as HTMLInputElement[];
    fireEvent.click(radios[1]);
    fireEvent.click(screen.getByTestId("nck-native-user-confirm"));
    const input = screen.getByTestId("nck-native-user-key") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "channel.program.status@channel=i:1" } });
    expect(screen.getByTestId("nck-native-key-error").textContent).toContain("native.");
    expect((screen.getByTestId("nck-native-add-point") as HTMLButtonElement).disabled).toBe(true);

    fireEvent.change(input, { target: { value: "native.C.1.21.1.397" } });
    expect(screen.getByTestId("nck-native-conflict").textContent).toContain("已被");
    expect((screen.getByTestId("nck-native-add-point") as HTMLButtonElement).disabled).toBe(true);
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("missing_required_native_fields_block_submit", () => {
    const point = nativePoint({ area_no: undefined });
    delete (point.parameters as Record<string, unknown>).area_no;
    render(
      <NckNativeFields schema={schema} takenPointKeys={[]} initialPoint={point} onSubmit={() => {}} />,
    );
    expect(screen.getByTestId("nck-native-issues").textContent).toContain("area_no");
    expect((screen.getByTestId("nck-native-add-point") as HTMLButtonElement).disabled).toBe(true);
  });

  it("existing_user_point_is_unconfirmed_and_requires_reconfirmation (A24)", () => {
    const onSubmit = vi.fn();
    const existing: NckPointSelection = {
      kind: "nck_native",
      point_key: "native.C.1.21.1.397",
      parameters: { ...nativePoint().parameters, identity_mode: "user" },
    };
    render(
      <NckNativeFields schema={schema} takenPointKeys={[]} initialPoint={existing} onSubmit={onSubmit} />,
    );
    const input = screen.getByTestId("nck-native-user-key") as HTMLInputElement;
    const confirm = screen.getByTestId("nck-native-user-confirm") as HTMLInputElement;
    const btn = screen.getByTestId("nck-native-add-point") as HTMLButtonElement;
    // 重开已有 user 点位：展示原 Key，但默认未确认，保存被禁用。
    expect(input.value).toBe("native.C.1.21.1.397");
    expect(confirm.checked).toBe(false);
    expect(btn.disabled).toBe(true);
    fireEvent.click(confirm);
    expect(btn.disabled).toBe(false);
    // 确认后再次修改 Key：确认立即撤销，必须重新确认。
    fireEvent.change(input, { target: { value: "native.C.1.21.1.398" } });
    expect(confirm.checked).toBe(false);
    expect(btn.disabled).toBe(true);
    fireEvent.click(confirm);
    expect(btn.disabled).toBe(false);
    fireEvent.click(btn);
    expect((onSubmit.mock.calls[0][0] as NckPointSelection).point_key).toBe(
      "native.C.1.21.1.398",
    );
  });

  it("switching_derived_and_back_to_user_requires_reconfirmation (A24)", () => {
    render(
      <NckNativeFields schema={schema} takenPointKeys={[]} initialPoint={nativePoint()} onSubmit={() => {}} />,
    );
    const radios = () => screen.getAllByRole("radio") as HTMLInputElement[];
    const confirm = () => screen.getByTestId("nck-native-user-confirm") as HTMLInputElement;
    const btn = () => screen.getByTestId("nck-native-add-point") as HTMLButtonElement;

    fireEvent.click(radios()[1]); // user
    const input = screen.getByTestId("nck-native-user-key") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "native.C.1.21.1.397" } });
    fireEvent.click(confirm());
    expect(btn().disabled).toBe(false);

    fireEvent.click(radios()[0]); // 回到 derived
    expect(screen.queryByTestId("nck-native-user-confirm")).toBeNull();
    fireEvent.click(radios()[1]); // 再回 user
    expect(confirm().checked).toBe(false);
    expect(btn().disabled).toBe(true);
    fireEvent.click(confirm());
    expect(btn().disabled).toBe(false);
  });
});
