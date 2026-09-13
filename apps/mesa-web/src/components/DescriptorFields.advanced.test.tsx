// Advanced 折叠单测：ui.advanced 纯展示行为，与协议/驱动无关。
import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { DescriptorFields } from "./DescriptorFields";
import type { FieldDescriptor, SchemaDescriptor } from "../types";

const f = (key: string, advanced?: boolean): FieldDescriptor => ({
  key,
  label: key,
  field_type: "string",
  required: false,
  validation: {},
  ui: advanced ? { advanced: true } : {},
});

const schema: SchemaDescriptor = { fields: [f("host"), f("port"), f("pdu", true), f("timeout", true)] };

describe("Advanced 折叠", () => {
  it("默认只渲染基础字段，高级收进折叠区", () => {
    render(<DescriptorFields schema={schema} value={{}} onChange={() => {}} />);
    expect(screen.getByText("host")).toBeTruthy();
    expect(screen.getByText("port")).toBeTruthy();
    expect(screen.queryByText("pdu")).toBeNull();
    expect(screen.queryByText("timeout")).toBeNull();
    expect(screen.getByText("▸ 高级（2）")).toBeTruthy();
  });

  it("点开展示高级字段，可再收起", async () => {
    const user = userEvent.setup();
    render(<DescriptorFields schema={schema} value={{}} onChange={() => {}} />);
    await user.click(screen.getByText("▸ 高级（2）"));
    expect(screen.getByText("pdu")).toBeTruthy();
    expect(screen.getByText("timeout")).toBeTruthy();
    await user.click(screen.getByText("▾ 收起高级"));
    expect(screen.queryByText("pdu")).toBeNull();
  });

  it("无高级字段时不渲染折叠入口", () => {
    render(<DescriptorFields schema={{ fields: [f("host")] }} value={{}} onChange={() => {}} />);
    expect(screen.queryByText(/高级/)).toBeNull();
  });
});
