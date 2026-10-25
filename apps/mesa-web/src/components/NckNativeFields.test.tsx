// PR-C 第三批：nck_native 表单（A24–A27）。
import { describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
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

// 「绕过 disabled」的 submit 探针：普通 fireEvent.click 会被 React 与 AntD 双重
// 拦截（disabled 表单元素不派发 click；AntD handleClick 在 mergedDisabled 时早退），
// 测不到 submit 内部的守卫。这里沿 React fiber 直接调用按钮组件级 onClick，精确
// 模拟「disabled 被移除 / 有人程序化调用 handler」。
//
// 难点：React 18 只在 DOM 节点首次挂载时缓存 fiber，多次 setState 后该 fiber 的
// memoizedProps 可能属于旧渲染，直接读会拿到过期的 confirmed/userKey 闭包。DOM 上
// 的 `__reactProps$*` 则每次 commit 都会更新，且与「当前」宿主 fiber 的 memoizedProps
// 是同一个对象；据此在 alternate 对里挑出当前 fiber，再向上找按钮组件。
interface FiberLike {
  type: unknown;
  memoizedProps: Record<string, unknown> | null;
  return: FiberLike | null;
  alternate: FiberLike | null;
}

/** 从 DOM 节点取「当前」宿主 fiber（避开挂载时缓存的过期 fiber）。 */
function currentHostFiber(button: HTMLButtonElement): FiberLike | null {
  const fiberKey = Object.keys(button).find((k) => k.startsWith("__reactFiber$"));
  if (!fiberKey) return null;
  const cached = (button as unknown as Record<string, FiberLike | undefined>)[fiberKey] ?? null;
  if (!cached) return null;
  const propsKey = Object.keys(button).find((k) => k.startsWith("__reactProps$"));
  if (!propsKey) return cached;
  const currentProps = (button as unknown as Record<string, Record<string, unknown> | undefined>)[
    propsKey
  ];
  if (
    currentProps &&
    cached.memoizedProps !== currentProps &&
    cached.alternate?.memoizedProps === currentProps
  ) {
    return cached.alternate;
  }
  return cached;
}

/** 触发按钮组件级 onClick（绕过 disabled）。返回是否真的调用到处理函数。 */
function invokeSubmitIgnoringDisabled(button: HTMLButtonElement): boolean {
  // 宿主 <button> 自身的 onClick 是 AntD 的内部处理函数，会在 mergedDisabled 处
  // 早退；必须从父级开始找组件级 fiber。
  let fiber = currentHostFiber(button)?.return ?? null;
  while (fiber) {
    const props = fiber.memoizedProps;
    if (typeof fiber.type !== "string" && props && typeof props.onClick === "function") {
      (props.onClick as (event: unknown) => void)({
        preventDefault() {},
        stopPropagation() {},
        nativeEvent: {},
      });
      return true;
    }
    fiber = fiber.return;
  }
  return false;
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

// P1 加固：UI 的 disabled 只是第一道门，submit 自身必须再复核 `confirmed`。
// 这里用上方的 fiber 探针直接调用按钮组件级 onClick，精确模拟「disabled 被回归
// 移除 / 有人程序化调用 handler」的场景；否则 AntD 与 React 会双重拦截禁用按钮
// 的 click，测试永远是假绿。
describe("P1：Native user 确认门（绕过 disabled 也无法提交未确认点位）", () => {
  /** 切到 user 身份（radio 顺序固定为 derived、user）。 */
  function switchToUser() {
    fireEvent.click(screen.getAllByRole("radio")[1]);
  }

  /** 程序化触发 submit：绕过 disabled 直接调用按钮组件级 onClick。 */
  function invokeSubmit() {
    const btn = screen.getByTestId("nck-native-add-point") as HTMLButtonElement;
    let invoked = false;
    act(() => {
      invoked = invokeSubmitIgnoringDisabled(btn);
    });
    // 探针必须真的调用到 submit；否则“零回调”可能只是因为什么都没发生。
    expect(invoked).toBe(true);
  }

  it("user 未确认：程序化调用 submit 不产出点位、回调零调用", () => {
    const onSubmit = vi.fn();
    render(
      <NckNativeFields schema={schema} takenPointKeys={[]} initialPoint={nativePoint()} onSubmit={onSubmit} />,
    );
    switchToUser();
    const btn = screen.getByTestId("nck-native-add-point") as HTMLButtonElement;
    // 前提：Key 合法且按钮确实 disabled（普通 UI 路径进不来）。
    expect(screen.getByTestId("nck-native-key").textContent).toBe("native.C.1.21.1.396");
    expect(btn.disabled).toBe(true);

    invokeSubmit();
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("user 已确认：同样程序化调用 submit 正常产出", () => {
    const onSubmit = vi.fn();
    render(
      <NckNativeFields schema={schema} takenPointKeys={[]} initialPoint={nativePoint()} onSubmit={onSubmit} />,
    );
    switchToUser();
    fireEvent.click(screen.getByTestId("nck-native-user-confirm"));
    expect((screen.getByTestId("nck-native-add-point") as HTMLButtonElement).disabled).toBe(false);

    invokeSubmit();
    expect(onSubmit).toHaveBeenCalledTimes(1);
    const point = onSubmit.mock.calls[0][0] as NckPointSelection;
    expect(point.kind).toBe("nck_native");
    expect(point.point_key).toBe("native.C.1.21.1.396");
    expect(point.parameters.identity_mode).toBe("user");
  });

  it("derived 模式本就不需要确认：程序化调用 submit 正常产出", () => {
    const onSubmit = vi.fn();
    render(
      <NckNativeFields schema={schema} takenPointKeys={[]} initialPoint={nativePoint()} onSubmit={onSubmit} />,
    );
    // derived 下没有确认勾选框；加固不得误伤这条路径。
    expect(screen.queryByTestId("nck-native-user-confirm")).toBeNull();
    expect((screen.getByTestId("nck-native-add-point") as HTMLButtonElement).disabled).toBe(false);

    invokeSubmit();
    expect(onSubmit).toHaveBeenCalledTimes(1);
    const point = onSubmit.mock.calls[0][0] as NckPointSelection;
    expect(point.point_key).toBe("native.C.1.21.1.396");
    expect(point.parameters).not.toHaveProperty("identity_mode");
  });

  it("确认后再改 Key 使确认失效：程序化调用 submit 仍被拒", () => {
    const onSubmit = vi.fn();
    render(
      <NckNativeFields schema={schema} takenPointKeys={[]} initialPoint={nativePoint()} onSubmit={onSubmit} />,
    );
    switchToUser();
    const input = screen.getByTestId("nck-native-user-key") as HTMLInputElement;
    const confirm = screen.getByTestId("nck-native-user-confirm") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "native.C.1.21.1.397" } });
    fireEvent.click(confirm);
    // 先证明“确认后本可提交”，否则后面的拒绝可能只是 Key 非法导致的。
    expect(confirm.checked).toBe(true);
    expect((screen.getByTestId("nck-native-add-point") as HTMLButtonElement).disabled).toBe(false);

    // 与既有 :126-130 的失效逻辑一致：Key 一变，上一次确认立即作废。
    fireEvent.change(input, { target: { value: "native.C.1.21.1.398" } });
    expect(confirm.checked).toBe(false);
    expect((screen.getByTestId("nck-native-add-point") as HTMLButtonElement).disabled).toBe(true);

    invokeSubmit();
    expect(onSubmit).not.toHaveBeenCalled();
  });
});

// A3：原生字段 Schema 里出现界面不认识的类型 ⇒ 不允许加入点位。
// 与通用 DescriptorFields 的拒绝逻辑同源，这里锁住表单层的提交门。
describe("A3：未知字段类型禁止加入原生点位", () => {
  const badSchema: SchemaDescriptor = {
    fields: [
      ...schema.fields,
      {
        key: "shape",
        label: "Shape",
        field_type: "geometry" as SchemaDescriptor["fields"][number]["field_type"],
        required: false,
        validation: {},
        ui: {},
      },
    ],
  };

  it("提示出现且「加入原生点位」禁用；绕过 disabled 也不 onSubmit", () => {
    const onSubmit = vi.fn();
    render(
      <NckNativeFields schema={badSchema} takenPointKeys={[]} initialPoint={nativePoint()} onSubmit={onSubmit} />,
    );
    expect(screen.getByTestId("descriptor-unsupported-notice")).toBeTruthy();
    const add = screen.getByTestId("nck-native-add-point") as HTMLButtonElement;
    expect(add.disabled).toBe(true);

    let invoked = false;
    act(() => {
      invoked = invokeSubmitIgnoringDisabled(add);
    });
    expect(invoked).toBe(true);
    expect(onSubmit).not.toHaveBeenCalled();
    expect(screen.getByTestId("nck-native-submit-error").textContent).toContain("当前界面不支持");
  });
});
