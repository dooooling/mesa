// PR26 Gate：Mock Driver 只给 Descriptor，Web 无代码修改即可生成合法
// ResourceSelection（typed 参数 + default 物化 + point_key 自动命名）。
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { ResourcePickerAntd, suggestPointKey } from "./ResourcePickerAntd";
import type { ResourceDescriptor } from "../types";

// 虚构协议 "mockbus"：真实代码库从未见过这些 resource/参数。
// encoding（required enum）与 scaled（required boolean）故意无 default：
// Core 允许 required 无 default，UI 不得显示假值（显示即保存）。
const MOCK_RESOURCES: ResourceDescriptor[] = [
  {
    id: "register",
    label: { default: "Register" },
    parameters: {
      fields: [
        { key: "unit", label: "Unit", field_type: "integer", required: true, default: 3, validation: { min: 0 }, ui: {} },
        { key: "encoding", label: "Encoding", field_type: "enum", required: true, validation: { enum_options: ["BE", "LE"] }, ui: {} },
        { key: "scaled", label: "Scaled", field_type: "boolean", required: true, validation: {}, ui: {} },
      ],
    },
    outputs: [
      { id: "value", label: { default: "Value" }, type_spec: { kind: "fixed", data_type: "F64" }, access: "read" },
      { id: "raw", label: { default: "Raw" }, type_spec: { kind: "fixed", data_type: "U32" }, access: "read" },
    ],
    modes: ["poll"],
  },
];

describe("suggestPointKey", () => {
  it("首选 resource.output，冲突递增 .2/.3", () => {
    expect(suggestPointKey("memory", "value", new Set())).toBe("memory.value");
    expect(suggestPointKey("memory", "value", new Set(["memory.value"]))).toBe("memory.value.2");
    expect(suggestPointKey("memory", "value", new Set(["memory.value", "memory.value.2"]))).toBe(
      "memory.value.3",
    );
  });
});

describe("ResourcePickerAntd（mock driver）", () => {
  it("无协议分支即可选型：typed 参数 + default 物化 + 无假值", () => {
    const onAdd = vi.fn((_sel: unknown): boolean => true);
    render(<ResourcePickerAntd resources={MOCK_RESOURCES} existingKeys={[]} onAdd={onAdd} />);
    // 按输出勾选（boolean 参数同样渲染 checkbox，必须用 aria-label 精确定位）
    const outputBox = (id: string) => {
      const el = screen.getByRole("checkbox", { name: `output-${id}` });
      return el.querySelector("input") ?? el;
    };
    fireEvent.click(outputBox("value"));
    fireEvent.click(outputBox("raw"));
    fireEvent.click(screen.getByText(/加\s*入/).closest("button")!);
    expect(onAdd).toHaveBeenCalledTimes(1);
    const sel = onAdd.mock.calls[0]![0] as {
      resource_id: string;
      parameters: Record<string, unknown>;
      outputs: Array<{ output: string; point_key: string }>;
    };
    expect(sel.resource_id).toBe("register");
    // default 物化为 typed 值（integer 3 为 number，非字符串）
    expect(sel.parameters.unit).toBe(3);
    // 无 default 的 required enum/boolean 不得凭空出现（显示即保存）
    expect("encoding" in sel.parameters).toBe(false);
    expect("scaled" in sel.parameters).toBe(false);
    // point_key 自动命名
    expect(sel.outputs).toEqual([
      { output: "value", point_key: "register.value" },
      { output: "raw", point_key: "register.raw" },
    ]);
  });

  it("同一 picker 连续加入三次：value/.2/.3（成功清 outputs、保留 params）", () => {
    const added: string[] = [];
    const onAdd = vi.fn((_sel: unknown): boolean => true);
    const { rerender } = render(
      <ResourcePickerAntd resources={MOCK_RESOURCES} existingKeys={added} onAdd={onAdd} />,
    );
    const addValue = () => {
      const el = screen.getByRole("checkbox", { name: "output-value" });
      fireEvent.click(el.querySelector("input") ?? el);
      fireEvent.click(screen.getByText(/加\s*入/).closest("button")!);
      const sel = onAdd.mock.calls[onAdd.mock.calls.length - 1]![0] as {
        outputs: Array<{ point_key: string }>;
      };
      for (const o of sel.outputs) added.push(o.point_key);
      rerender(<ResourcePickerAntd resources={MOCK_RESOURCES} existingKeys={added} onAdd={onAdd} />);
    };
    addValue();
    addValue();
    addValue();
    expect(onAdd).toHaveBeenCalledTimes(3);
    expect(added).toEqual(["register.value", "register.value.2", "register.value.3"]);
  });

  it("父层拒绝时不清空（用户手改撞车可继续编辑）", () => {
    const onAdd = vi.fn((_sel: unknown): boolean => false);
    render(<ResourcePickerAntd resources={MOCK_RESOURCES} existingKeys={[]} onAdd={onAdd} />);
    const el = screen.getByRole("checkbox", { name: "output-value" });
    fireEvent.click(el.querySelector("input") ?? el);
    fireEvent.click(screen.getByText(/加\s*入/).closest("button")!);
    // 拒绝后 outputs 保留：point_key 输入框仍在
    expect(screen.getByDisplayValue("register.value")).toBeTruthy();
  });
});
