// PR27 Blocker 3 回归：Secret marker（{secret_set:true}）不得当成
// plaintext 渲染或写回——显示空值、占位提示保持不变；未触碰时 marker
// 原样保留；用户键入后 marker 被明文替换。
import { describe, expect, it, vi } from "vitest";
import { useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { DescriptorFields } from "./DescriptorFields";
import type { FieldDescriptor, SchemaDescriptor } from "../types";

const secretField = (placeholder?: string): FieldDescriptor => ({
  key: "password",
  label: "密码",
  field_type: "secret",
  required: false,
  validation: {},
  ui: { placeholder },
});

const schema = (f: FieldDescriptor): SchemaDescriptor => ({ fields: [f] });

describe("Secret marker 渲染", () => {
  it("marker 显示空值 + 保持不变占位，不伪显示密码", () => {
    render(
      <DescriptorFields
        schema={schema(secretField("请输入密码"))}
        value={{ password: { secret_set: true } }}
        onChange={() => {}}
      />,
    );
    const input = document.querySelector('input[type="password"]') as HTMLInputElement;
    expect(input.value).toBe("");
    expect(input.placeholder).toBe("已设置，留空保持不变");
    expect(input.value).not.toContain("object");
  });

  it("明文值正常显示（编辑回显路径不受影响）", () => {
    render(
      <DescriptorFields
        schema={schema(secretField("请输入密码"))}
        value={{ password: "abc" }}
        onChange={() => {}}
      />,
    );
    const input = document.querySelector('input[type="password"]') as HTMLInputElement;
    expect(input.value).toBe("abc");
  });

  it("用户键入新密码后 marker 被明文替换（onChange 收到 string）", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    // 受控组件需 state 承接 onChange，否则每次键入都基于旧值从空起算
    function Harness() {
      const [v, setV] = useState<Record<string, unknown>>({ password: { secret_set: true } });
      return (
        <DescriptorFields
          schema={schema(secretField())}
          value={v}
          onChange={(next) => {
            setV(next);
            onChange(next);
          }}
        />
      );
    }
    render(<Harness />);
    const input = document.querySelector('input[type="password"]') as HTMLInputElement;
    await user.type(input, "new-secret");
    expect(input.value).toBe("new-secret");
    expect(onChange).toHaveBeenCalled();
    const last = onChange.mock.calls[onChange.mock.calls.length - 1][0] as Record<string, unknown>;
    expect(last.password).toBe("new-secret");
  });

  it("未触碰时 onChange 不触发（marker 不会被显示产物污染）", () => {
    const onChange = vi.fn();
    render(
      <DescriptorFields
        schema={schema(secretField())}
        value={{ password: { secret_set: true } }}
        onChange={onChange}
      />,
    );
    expect(screen.getByPlaceholderText("已设置，留空保持不变")).toBeTruthy();
    expect(onChange).not.toHaveBeenCalled();
  });
});
