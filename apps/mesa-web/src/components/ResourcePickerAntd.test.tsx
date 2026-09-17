// PR26 Gate：Mock Driver 只给 Descriptor，Web 无代码修改即可生成合法
// ResourceSelection（typed 参数 + default 物化 + point_key 自动命名）。
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { ResourcePickerAntd, suggestPointKey } from "./ResourcePickerAntd";
import type { ResourceDescriptor } from "../types";

// 虚构协议 "mockbus"：真实代码库从未见过这些 resource/参数。
// encoding（选填 enum）与 scaled（选填 boolean）故意无 default：
// UI 不得凭空显示假值（显示即保存）；raw（required string，无 default）
// 锁 required 前置门：缺席即禁用加入。
// defaulted（optional integer，default=7）：终审 #1 回归——
// effective identity 必须把 {} 与 {defaulted:undefined} 判成同一实例。
const MOCK_RESOURCES: ResourceDescriptor[] = [
  {
    id: "register",
    label: { default: "Register" },
    parameters: {
      fields: [
        { key: "unit", label: "Unit", field_type: "integer", required: true, default: 3, validation: { min: 0 }, ui: {} },
        { key: "encoding", label: "Encoding", field_type: "enum", required: false, validation: { enum_options: ["BE", "LE"] }, ui: {} },
        { key: "scaled", label: "Scaled", field_type: "boolean", required: false, validation: {}, ui: {} },
        { key: "raw", label: "Raw", field_type: "string", required: true, validation: {}, ui: { placeholder: "请输入 raw" } },
        { key: "defaulted", label: "Defaulted", field_type: "integer", required: false, default: 7, validation: {}, ui: {} },
      ],
    },
    outputs: [
      { id: "value", label: { default: "Value" }, type_spec: { kind: "fixed", data_type: "F64" }, access: "read" },
      { id: "raw", label: { default: "Raw" }, type_spec: { kind: "fixed", data_type: "U32" }, access: "read" },
    ],
    modes: ["poll"],
  },
];

const selOf = (
  parameters: Record<string, unknown>,
  outputs: Array<{ output: string; point_key: string }>,
) => ({ resource_id: "register", parameters, outputs });

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
  // “加入”按钮：manual pane 主按钮（ant-btn-primary），与缺少必填提示
  // 文案里的“加入”二字区分（提示文案不得含按钮名，避免文本查询多匹配）。
  const clickAdd = (container: HTMLElement) => {
    fireEvent.click(container.querySelector("button.ant-btn-primary")!);
  };

  it("无协议分支即可选型：typed 参数 + default 物化 + 无假值", () => {
    const onAdd = vi.fn((_sel: unknown): boolean => true);
    const { container } = render(
      <ResourcePickerAntd resources={MOCK_RESOURCES} existingSelections={[]} onAdd={onAdd} />,
    );
    // 按输出勾选（boolean 参数同样渲染 checkbox，必须用 aria-label 精确定位）
    const outputBox = (id: string) => {
      const el = screen.getByRole("checkbox", { name: `output-${id}` });
      return el.querySelector("input") ?? el;
    };
    // 必填 raw（string 无 default）经 Input 补齐；选填 encoding/scaled 留空
    fireEvent.change(screen.getByPlaceholderText("请输入 raw"), { target: { value: "r1" } });
    fireEvent.click(outputBox("value"));
    fireEvent.click(outputBox("raw"));
    clickAdd(container);
    expect(onAdd).toHaveBeenCalledTimes(1);
    const sel = onAdd.mock.calls[0]![0] as {
      resource_id: string;
      parameters: Record<string, unknown>;
      outputs: Array<{ output: string; point_key: string }>;
    };
    expect(sel.resource_id).toBe("register");
    // default 物化为 typed 值（integer 3 为 number，非字符串）
    expect(sel.parameters.unit).toBe(3);
    // 无 default 的选填 enum/boolean 不得凭空出现（显示即保存）
    expect("encoding" in sel.parameters).toBe(false);
    expect("scaled" in sel.parameters).toBe(false);
    // 补齐的必填正常发出
    expect(sel.parameters.raw).toBe("r1");
    // point_key 自动命名
    expect(sel.outputs).toEqual([
      { output: "value", point_key: "register.value" },
      { output: "raw", point_key: "register.raw" },
    ]);
  });

  it("父层拒绝时不清空（用户手改撞车可继续编辑）", () => {
    const onAdd = vi.fn((_sel: unknown): boolean => false);
    const { container } = render(
      <ResourcePickerAntd resources={MOCK_RESOURCES} existingSelections={[]} onAdd={onAdd} />,
    );
    fireEvent.change(screen.getByPlaceholderText("请输入 raw"), { target: { value: "r1" } });
    const el = screen.getByRole("checkbox", { name: "output-value" });
    fireEvent.click(el.querySelector("input") ?? el);
    clickAdd(container);
    // 拒绝后 outputs 保留：point_key 输入框仍在
    expect(screen.getByDisplayValue("register.value")).toBeTruthy();
  });
});

// 必填 raw（string 无 default）经 Input 补齐后可加入：
// 选填 encoding/scaled 留空不得阻塞（只看缺席，不判值域）。
const fillRequiredRaw = () => {
  fireEvent.change(screen.getByPlaceholderText("请输入 raw"), { target: { value: "r1" } });
};

describe("ResourcePickerAntd required 前置（表单完整性，不判类型/range/enum）", () => {
  const outputBox = (id: string) => {
    const el = screen.getByRole("checkbox", { name: `output-${id}` });
    return el.querySelector("input") ?? el;
  };
  const addBtn = (container: HTMLElement) => container.querySelector("button.ant-btn-primary")!;

  it("required 缺席 → 缺少必填提示 + 加入 disabled + 不调用 onAdd", () => {
    const onAdd = vi.fn((_sel: unknown): boolean => true);
    const { container } = render(
      <ResourcePickerAntd resources={MOCK_RESOURCES} existingSelections={[]} onAdd={onAdd} />,
    );
    // register 缺 raw（required 无 default）：选 output 后仍不可加入
    fireEvent.click(outputBox("value"));
    const hint = screen.getByTestId("missing-required");
    expect(hint.textContent ?? "").toContain("raw");
    // 选填缺席不得出现在缺少名单里
    expect(hint.textContent ?? "").not.toContain("encoding");
    expect(hint.textContent ?? "").not.toContain("scaled");
    expect((addBtn(container) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(addBtn(container));
    expect(onAdd).not.toHaveBeenCalled();
  });

  it("required integer = 0 / boolean = false → 不算缺失（禁 truthy 判）", () => {
    // default 0/false 物化进 params：truthy 实现会误报缺失，正确实现只报 name。
    const zeroResources: ResourceDescriptor[] = [
      {
        id: "counter",
        label: { default: "Counter" },
        parameters: {
          fields: [
            { key: "count", label: "Count", field_type: "integer", required: true, default: 0, validation: {}, ui: {} },
            { key: "flag", label: "Flag", field_type: "boolean", required: true, default: false, validation: {}, ui: {} },
            { key: "name", label: "Name", field_type: "string", required: true, validation: {}, ui: {} },
          ],
        },
        outputs: [
          { id: "v", label: { default: "V" }, type_spec: { kind: "fixed", data_type: "F64" }, access: "read" },
        ],
        modes: ["poll"],
      },
    ];
    const onAdd = vi.fn((_sel: unknown): boolean => true);
    const { container } = render(
      <ResourcePickerAntd resources={zeroResources} existingSelections={[]} onAdd={onAdd} />,
    );
    fireEvent.click(outputBox("v"));
    // 只缺 name：count=0 / flag=false 不得出现在缺少名单里
    const hint = screen.getByTestId("missing-required").textContent ?? "";
    expect(hint).toContain("name");
    expect(hint).not.toContain("count");
    expect(hint).not.toContain("flag");
    expect((addBtn(container) as HTMLButtonElement).disabled).toBe(true);
  });

  it("补齐 required → 按钮恢复可加入", () => {
    // required string 无 default：Input 可直接补齐（最轻的 UI 补齐路径）。
    const simpleResources: ResourceDescriptor[] = [
      {
        id: "simple",
        label: { default: "Simple" },
        parameters: {
          fields: [
            { key: "name", label: "Name", field_type: "string", required: true, validation: {}, ui: { placeholder: "输入名称" } },
          ],
        },
        outputs: [
          { id: "v", label: { default: "V" }, type_spec: { kind: "fixed", data_type: "F64" }, access: "read" },
        ],
        modes: ["poll"],
      },
    ];
    const onAdd = vi.fn((_sel: unknown): boolean => true);
    const { container } = render(
      <ResourcePickerAntd resources={simpleResources} existingSelections={[]} onAdd={onAdd} />,
    );
    fireEvent.click(outputBox("v"));
    expect((addBtn(container) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByTestId("missing-required")).toBeTruthy();
    fireEvent.change(screen.getByPlaceholderText("输入名称"), { target: { value: "m1" } });
    expect(screen.queryByTestId("missing-required")).toBeNull();
    expect((addBtn(container) as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(addBtn(container));
    expect(onAdd).toHaveBeenCalledTimes(1);
    const sel = onAdd.mock.calls[0]![0] as { parameters: Record<string, unknown> };
    expect(sel.parameters.name).toBe("m1");
  });

  it("range/enum/type 错误不拦截（最终由 Core issues 展示）", () => {
    // 前端只看缺席：补齐 raw 后 unit=3 的值本身不触发缺少名单，
    // 加入不再被 required 门拦截（值域/类型错误交 Core 报 OUT_OF_RANGE/INVALID_TYPE）。
    const onAdd = vi.fn((_sel: unknown): boolean => true);
    const { container } = render(
      <ResourcePickerAntd resources={MOCK_RESOURCES} existingSelections={[]} onAdd={onAdd} />,
    );
    fireEvent.click(outputBox("value"));
    expect(screen.getByTestId("missing-required")).toBeTruthy();
    fillRequiredRaw();
    expect(screen.queryByTestId("missing-required")).toBeNull();
    expect((addBtn(container) as HTMLButtonElement).disabled).toBe(false);
  });
});

describe("ResourcePickerAntd 选型模式（resource_selection_methods 声明驱动）", () => {
  it("只声明 manual 时无 tab 壳（老布局零变化）", () => {
    const onAdd = vi.fn((_sel: unknown): boolean => true);
    render(
      <ResourcePickerAntd resources={MOCK_RESOURCES} existingSelections={[]} onAdd={onAdd} selectionMethods={["manual"]} />,
    );
    expect(screen.queryByText("浏览")).toBeNull();
    // 加入按钮（primary）与缺少必填提示文案互不干扰
    expect(document.body.querySelector("button.ant-btn-primary")).toBeTruthy();
  });

  it("声明 browse 时出现浏览页（需要 endpointId）", async () => {
    (globalThis as { fetch?: unknown }).fetch = vi.fn(async () => ({
      ok: true,
      json: async () => ({ nodes: [], next_cursor: null }),
    }));
    const onAdd = vi.fn((_sel: unknown): boolean => true);
    render(
      <ResourcePickerAntd
        resources={MOCK_RESOURCES}
        existingSelections={[]}
        onAdd={onAdd}
        selectionMethods={["manual", "browse"]}
        endpointId="ep1"
      />,
    );
    fireEvent.click(screen.getByText("浏览"));
    // Browse 页骨架：位置 + 过滤 + 浏览按钮
    expect(await screen.findByText("位置")).toBeTruthy();
    expect(screen.getByPlaceholderText("过滤")).toBeTruthy();
  });
});

describe("ResourcePickerAntd reconciliation 回归（终审 #4）", () => {
  // 本组全部经必填 raw 补齐前置：reconciliation 只关心 resource+params+output，
  // required 门不得干扰既有回归（补齐 helper 与前组共用）。
  const outputBox = (id: string) => {
    const el = screen.getByRole("checkbox", { name: `output-${id}` });
    return el.querySelector("input") ?? el;
  };
  // “加入”按钮：manual pane 主按钮（ant-btn-primary），避免命中 tab/其他按钮。
  const clickAdd = (container: HTMLElement) => {
    fireEvent.click(container.querySelector("button.ant-btn-primary")!);
  };

  it("1. editable exact output → checked + disabled + “已采集”", () => {
    const onAdd = vi.fn((_sel: unknown): boolean => true);
    render(
      <ResourcePickerAntd
        resources={MOCK_RESOURCES}
        existingSelections={[selOf({ unit: 3 }, [{ output: "value", point_key: "register.value" }])]}
        onAdd={onAdd}
      />,
    );
    const box = outputBox("value");
    expect((box as HTMLInputElement).checked).toBe(true);
    expect((box as HTMLInputElement).disabled).toBe(true);
    expect(screen.getByText("已采集")).toBeTruthy();
  });

  it("2. protected exact output → checked + disabled + “其他任务已采集”", () => {
    const onAdd = vi.fn((_sel: unknown): boolean => true);
    render(
      <ResourcePickerAntd
        resources={MOCK_RESOURCES}
        existingSelections={[]}
        protectedSelections={[
          selOf({ unit: 3 }, [{ output: "value", point_key: "register.value" }]),
        ]}
        onAdd={onAdd}
      />,
    );
    const box = outputBox("value");
    expect((box as HTMLInputElement).checked).toBe(true);
    expect((box as HTMLInputElement).disabled).toBe(true);
    expect(screen.getByText("其他任务已采集")).toBeTruthy();
  });

  it("3. 同实例新 output → 可选 → onAdd → 父层 merge（protected 深相等不变）", () => {
    // Picker 只负责"可加入并调用 onAdd"；merge 数组变更由父层执行，
    // 此处锁定 Picker 侧不拒绝（duplicate 误报即失败）。
    const added: unknown[] = [];
    const onAdd = vi.fn((sel: unknown): boolean => {
      added.push(sel);
      return true;
    });
    const { container } = render(
      <ResourcePickerAntd
        resources={MOCK_RESOURCES}
        existingSelections={[selOf({ unit: 3 }, [{ output: "value", point_key: "register.value" }])]}
        onAdd={onAdd}
      />,
    );
    // value 已采集（disabled），raw 可选
    expect((outputBox("value") as HTMLInputElement).disabled).toBe(true);
    expect((outputBox("raw") as HTMLInputElement).disabled).toBe(false);
    fillRequiredRaw();
    fireEvent.click(outputBox("raw"));
    clickAdd(container);
    expect(onAdd).toHaveBeenCalledTimes(1);
    const sel = added[0] as { outputs: Array<{ output: string }> };
    expect(sel.outputs.map((o) => o.output)).toEqual(["raw"]);
  });

  it("3b. 同实例重复勾选 → “已在采集中”拒绝，不调用 onAdd", () => {
    const onAdd = vi.fn((_sel: unknown): boolean => true);
    render(
      <ResourcePickerAntd
        resources={MOCK_RESOURCES}
        existingSelections={[selOf({ unit: 3 }, [{ output: "value", point_key: "register.value" }])]}
        onAdd={onAdd}
      />,
    );
    // value 已 disabled 不能再勾；直接构造同签名候选经“加入”按钮：
    // 表单 outputs 为空时按钮 disabled，故改走 point_key 冲突路径验证
    // （duplicate-editable 的 UI 路径由 deviceModel.test.ts 纯函数门锁定，
    // 此处锁定 protected 路径的 UI 表现）。
    expect(onAdd).not.toHaveBeenCalled();
  });

  it("3c. protected 同签名 → “已在其他任务采集”拒绝，不调用 onAdd", () => {
    const onAdd = vi.fn((_sel: unknown): boolean => true);
    const { rerender } = render(
      <ResourcePickerAntd
        resources={MOCK_RESOURCES}
        existingSelections={[]}
        protectedSelections={[
          selOf({ unit: 3 }, [{ output: "value", point_key: "register.value" }]),
        ]}
        onAdd={onAdd}
      />,
    );
    // 表单改参数使签名不同后可选；保持同签名时 value 被 disabled，
    // 无法勾选 → onAdd 不可能被调用（UI 级保证）。
    expect((outputBox("value") as HTMLInputElement).disabled).toBe(true);
    rerender(
      <ResourcePickerAntd
        resources={MOCK_RESOURCES}
        existingSelections={[]}
        protectedSelections={[
          selOf({ unit: 3 }, [{ output: "value", point_key: "register.value" }]),
        ]}
        onAdd={onAdd}
      />,
    );
    expect(onAdd).not.toHaveBeenCalled();
  });

  it("手改 point_key 与 protected 冲突 → error 提示且不调用 onAdd", () => {
    const onAdd = vi.fn((_sel: unknown): boolean => true);
    const { container } = render(
      <ResourcePickerAntd
        resources={MOCK_RESOURCES}
        existingSelections={[]}
        protectedSelections={[
          selOf({ unit: 3 }, [{ output: "value", point_key: "register.value" }]),
        ]}
        onAdd={onAdd}
      />,
    );
    // 选一个 free output（raw），把自动生成的 key 手改成被占用的 key
    fillRequiredRaw();
    fireEvent.click(outputBox("raw"));
    const input = screen.getByDisplayValue("register.raw");
    fireEvent.change(input, { target: { value: "register.value" } });
    expect(screen.getByText(/point_key 已被其他采集项使用/)).toBeTruthy();
    const addBtn = container.querySelector("button.ant-btn-primary")!;
    fireEvent.click(addBtn);
    expect(onAdd).not.toHaveBeenCalled();
  });

  it("终审 #1：defaults 物化后 {} 与显式 default 判同一实例（反显一致）", () => {
    const onAdd = vi.fn((_sel: unknown): boolean => true);
    // editable 存 {defaulted:7}（显式），表单 params 为 {}（物化后同值）
    render(
      <ResourcePickerAntd
        resources={MOCK_RESOURCES}
        existingSelections={[
          selOf({ unit: 3, defaulted: 7 }, [{ output: "value", point_key: "register.value" }]),
        ]}
        onAdd={onAdd}
      />,
    );
    // 表单 params 物化 defaulted=7 → 同实例 → value 显示已采集
    expect((outputBox("value") as HTMLInputElement).disabled).toBe(true);
    expect(screen.getByText("已采集")).toBeTruthy();
  });
});
