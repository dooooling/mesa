// A3 组件门禁：未知 field_type 不得渲染成可编辑文本框；兼容性提示只出现一次，
// 字段名/类型进「技术详情」；高级区折叠不能掩盖不支持。
import { describe, expect, it, vi } from "vitest";
import { useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { DescriptorFields } from "./DescriptorFields";
import type { FieldDescriptor, SchemaDescriptor } from "../types";
import { UNSUPPORTED_CONFIG_NOTICE } from "../descriptorCompatibility";

function field(
  key: string,
  fieldType: string,
  ui: FieldDescriptor["ui"] = {},
  placeholder?: string,
): FieldDescriptor {
  return {
    key,
    label: key.toUpperCase(),
    field_type: fieldType as FieldDescriptor["field_type"],
    required: false,
    validation: {},
    ui: { ...ui, placeholder },
  };
}

const schema = (fields: FieldDescriptor[]): SchemaDescriptor => ({ fields });

describe("未知字段类型：拒绝编辑", () => {
  const unknownSchema = schema([field("host", "host", {}, "主机"), field("shape", "geometry")]);

  it("打开界面即出现一次兼容性提示（不是逐行红框）", () => {
    render(<DescriptorFields schema={unknownSchema} value={{ host: "h", shape: { x: 1 } }} onChange={() => {}} />);
    expect(screen.getAllByTestId("descriptor-unsupported-notice")).toHaveLength(1);
    expect(screen.getByText(UNSUPPORTED_CONFIG_NOTICE)).toBeTruthy();
    // 字段名与类型只在「技术详情」里出现一次
    expect(screen.getByText("shape · geometry")).toBeTruthy();
  });

  it("未知类型字段是只读视图：没有输入框，原值不被强转字符串", () => {
    const { container } = render(
      <DescriptorFields schema={unknownSchema} value={{ host: "h", shape: { x: 1 } }} onChange={() => {}} />,
    );
    const block = screen.getByTestId("descriptor-unsupported-field-shape");
    expect(block.querySelector("input")).toBeNull();
    expect(block.textContent).toContain('"x": 1');
    expect(block.textContent).not.toContain("[object Object]");
    // 已知文本类字段仍是输入框
    expect(container.querySelector('[data-field-key="host"] input')).toBeTruthy();
  });

  it("存在未知类型时其它字段也禁用（不能边报错边继续改）", () => {
    render(<DescriptorFields schema={unknownSchema} value={{ host: "h" }} onChange={() => {}} />);
    const host = screen.getByPlaceholderText("主机") as HTMLInputElement;
    expect(host.disabled).toBe(true);
  });

  it("只读视图不产生 onChange（值原样保留）", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <DescriptorFields schema={unknownSchema} value={{ host: "h", shape: { x: 1 } }} onChange={onChange} />,
    );
    await user.click(screen.getByTestId("descriptor-unsupported-field-shape"));
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe("未知字段：不丢弃（保留原值）", () => {
  it("保存其它字段时，Schema 未声明的键原样保留在 onChange 结果里", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    // Schema 只认识 known；配置里还带着 mystery（例如旧版本/新版本驱动的字段）。
    function Harness() {
      const [value, setValue] = useState<Record<string, unknown>>({
        known: "a",
        mystery: { x: 1 },
      });
      return (
        <DescriptorFields
          schema={schema([field("known", "string", {}, "已知")])}
          value={value}
          onChange={(next) => {
            onChange(next);
            setValue(next);
          }}
        />
      );
    }
    render(<Harness />);
    await user.type(screen.getByPlaceholderText("已知"), "b");
    expect(onChange).toHaveBeenCalled();
    const calls = onChange.mock.calls;
    const last = calls[calls.length - 1]?.[0] as Record<string, unknown>;
    // 未知键不得被删除，也不得被强转成字符串
    expect(last.mystery).toEqual({ x: 1 });
  });
});

describe("未知字段类型：高级区折叠不能掩盖", () => {
  const advancedSchema = schema([
    field("host", "host", {}, "主机"),
    field("shape", "geometry", { advanced: true }),
  ]);

  it("未展开高级区就已提示不支持，且高级字段标为（高级）", () => {
    const { container } = render(<DescriptorFields schema={advancedSchema} value={{ host: "h" }} onChange={() => {}} />);
    // 折叠区未展开：高级字段本体不渲染，但提示已经出现
    expect(container.querySelector('[data-field-key="shape"]')).toBeNull();
    expect(screen.getByTestId("descriptor-unsupported-notice")).toBeTruthy();
    expect(screen.getByText("shape · geometry（高级）")).toBeTruthy();
  });

  it("展开高级区后未知字段仍是只读视图（不是文本框）", async () => {
    const user = userEvent.setup();
    const { container } = render(
      <DescriptorFields schema={advancedSchema} value={{ host: "h", shape: { x: 1 } }} onChange={() => {}} />,
    );
    await user.click(screen.getByText("▸ 高级（1）"));
    const block = screen.getByTestId("descriptor-unsupported-field-shape");
    expect(block.querySelector("input")).toBeNull();
    expect(container.querySelector('[data-field-key="shape"] input')).toBeNull();
  });
});

describe("已知类型不受影响", () => {
  it("文本类类型白名单共用文本框，不出现兼容性提示", () => {
    const known = schema([
      field("a", "string", {}, "a"),
      field("b", "host", {}, "b"),
      field("c", "url", {}, "c"),
      field("d", "file", {}, "d"),
      field("e", "certificate_ref", {}, "e"),
      field("f", "integer", {}),
    ]);
    const { container } = render(<DescriptorFields schema={known} value={{}} onChange={() => {}} />);
    expect(screen.queryByTestId("descriptor-unsupported-notice")).toBeNull();
    for (const key of ["a", "b", "c", "d", "e", "f"]) {
      expect(container.querySelector(`[data-field-key="${key}"] input`)).toBeTruthy();
    }
  });
});
