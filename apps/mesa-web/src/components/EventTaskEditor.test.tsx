// PR8 Gate（组件）：EventTask Editor 纯 Descriptor 驱动；只发 mesa.events.v1；
// 私有绑定只读可删；运行中只读；409 保留表单。
import { describe, expect, it, vi, beforeEach, type Mock } from "vitest";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { api } from "../api";
import { EventTaskEditor } from "./EventTaskEditor";

vi.mock("../api", () => ({
  api: {
    listEndpoints: vi.fn(),
    getDescriptor: vi.fn(),
    listEventTasks: vi.fn(),
    replaceEventTasks: vi.fn(),
  },
  isEventStoreUnavailable: () => false,
}));

const mocked = api as unknown as {
  listEndpoints: Mock;
  getDescriptor: Mock;
  listEventTasks: Mock;
  replaceEventTasks: Mock;
};

const DESCRIPTOR = {
  contract_major: 1,
  contract_minor: 0,
  identity: { driver_id: "drv1", name: "D", version: "1" },
  connection: { fields: [] },
  resources: [],
  controls: { commands: [] },
  discovery: { manual: true, browse: false, import: false },
  capabilities: { poll: true, subscribe: true, browse: false, write: false, method: false, events: true },
  events: {
    streams: [
      {
        id: "stream.counter",
        label: { default: "Counter" },
        modes: ["poll", "subscribe"],
        parameters: { fields: [] },
        fields: [{ key: "value", label: { default: "Value" }, data_type: "u64" }],
      },
      {
        id: "stream.alarm",
        label: { default: "Alarm" },
        modes: ["subscribe"],
        parameters: {
          fields: [
            {
              key: "limit",
              label: "Limit",
              field_type: "integer",
              required: true,
              validation: {},
              ui: {},
            },
          ],
        },
        fields: [],
      },
    ],
  },
};

const GENERIC_TASK = {
  id: "ev-counter",
  mode: "subscribe",
  interval_ms: null,
  binding: { kind: "mesa.events.v1", config: { stream_id: "stream.counter", parameters: {} } },
};

const LEGACY_TASK = {
  id: "old-private",
  mode: "subscribe",
  interval_ms: null,
  binding: { kind: "legacy.private", config: { secret: "x" } },
};

function mockStopped(eventTasks: unknown[] = [GENERIC_TASK, LEGACY_TASK]) {
  mocked.listEndpoints.mockResolvedValue({ endpoints: [{ id: "ep1", driver_id: "drv1" }] });
  mocked.getDescriptor.mockResolvedValue(DESCRIPTOR);
  mocked.listEventTasks.mockResolvedValue({ endpoint_id: "ep1", revision: 1, event_tasks: eventTasks });
  mocked.replaceEventTasks.mockResolvedValue({ revision: 2 });
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("EventTaskEditor", () => {
  it("按 descriptor.events 渲染流与字段提示", async () => {
    mockStopped();
    render(<EventTaskEditor />);
    // 已有标准任务的 Stream 选择显示 descriptor label + id
    expect(await screen.findByText("Counter (stream.counter)")).toBeTruthy();
    // 流声明字段作为提示（非动态猜列）
    expect(await screen.findByText(/该流可能产生的字段：value/)).toBeTruthy();
  });

  it("切换流后模式跟随 stream.modes（subscribe 流隐藏 Interval）", async () => {
    const user = userEvent.setup();
    mockStopped([GENERIC_TASK]);
    render(<EventTaskEditor />);
    await screen.findByText("Counter (stream.counter)");
    await user.click(screen.getByText("Counter (stream.counter)"));
    await user.click(await screen.findByText("Alarm (stream.alarm)"));
    await waitFor(() => expect(screen.queryByText("Interval ms")).toBeNull());
    // subscribe-only 流：模式选择只剩 subscribe
    expect(screen.getByText("Limit")).toBeTruthy();
  });

  it("Poll 缺 interval 报错且禁用保存", async () => {
    const user = userEvent.setup();
    mockStopped([
      { id: "ev-poll", mode: "poll", interval_ms: 100, binding: { kind: "mesa.events.v1", config: { stream_id: "stream.counter", parameters: {} } } },
    ]);
    render(<EventTaskEditor />);
    await screen.findByText("Counter (stream.counter)");
    const spin = screen.getByRole("spinbutton");
    await user.clear(spin);
    expect(await screen.findByText("Poll 模式必须提供正整数 interval_ms")).toBeTruthy();
    const save = screen.getByRole("button", { name: "保存订阅" });
    expect((save as HTMLButtonElement).disabled).toBe(true);
  });

  it("私有绑定只读展示、可删除", async () => {
    const user = userEvent.setup();
    mockStopped([LEGACY_TASK]);
    render(<EventTaskEditor />);
    expect(await screen.findByText("Legacy / Private Binding")).toBeTruthy();
    expect(screen.getByText("legacy.private")).toBeTruthy();
    const card = screen.getByText("old-private").closest(".ant-card") as HTMLElement;
    const del = within(card).getByRole("button", { name: /删除/ });
    expect((del as HTMLButtonElement).disabled).toBe(false);
    await user.click(del);
    await waitFor(() => expect(screen.queryByText("Legacy / Private Binding")).toBeNull());
  });

  it("运行中 Endpoint 只读、保存禁用", async () => {
    mockStopped([GENERIC_TASK]);
    mocked.listEndpoints.mockResolvedValue({
      endpoints: [{ id: "ep1", driver_id: "drv1", runtime: { state: "RUNNING" } }],
    });
    render(<EventTaskEditor />);
    await screen.findByText("运行中 · 只读");
    expect(screen.getByText("事件任务只能在 Endpoint 停止状态修改")).toBeTruthy();
    const save = screen.getByRole("button", { name: "保存订阅" });
    expect((save as HTMLButtonElement).disabled).toBe(true);
  });

  it("PUT body 只含 mesa.events.v1（无私有 kind）", async () => {
    const user = userEvent.setup();
    mockStopped([GENERIC_TASK]);
    render(<EventTaskEditor />);
    await screen.findByText("Counter (stream.counter)");
    await user.click(screen.getByRole("button", { name: "保存订阅" }));
    await waitFor(() => expect(mocked.replaceEventTasks).toHaveBeenCalledTimes(1));
    const [endpointId, tasks] = mocked.replaceEventTasks.mock.calls[0] as [string, typeof GENERIC_TASK[]];
    expect(endpointId).toBe("ep1");
    expect(tasks).toHaveLength(1);
    expect(tasks[0].binding.kind).toBe("mesa.events.v1");
    expect(tasks[0].binding.config).toEqual({ stream_id: "stream.counter", parameters: {} });
  });

  it("descriptor.events 为空显示空状态", async () => {
    mockStopped([]);
    mocked.getDescriptor.mockResolvedValue({ ...DESCRIPTOR, events: { streams: [] } });
    render(<EventTaskEditor />);
    expect(await screen.findByText("该驱动未声明事件流")).toBeTruthy();
  });

  it("P0-2：切 Endpoint 后旧请求被忽略，保存写新设备的任务", async () => {
    const user = userEvent.setup();
    const taskEp1 = { ...GENERIC_TASK, id: "task-ep1" };
    const taskEp2 = { ...GENERIC_TASK, id: "task-ep2" };
    mocked.listEndpoints.mockResolvedValue({
      endpoints: [
        { id: "ep1", driver_id: "drv1" },
        { id: "ep2", driver_id: "drv1" },
      ],
    });
    mocked.getDescriptor.mockResolvedValue(DESCRIPTOR);
    // 两端点的任务请求都挂起，由测试按序放行
    const resolvers: Record<string, () => void> = {};
    mocked.listEventTasks.mockImplementation(
      (id: string) =>
        new Promise((resolve) => {
          resolvers[id] = () =>
            resolve({
              endpoint_id: id,
              revision: 1,
              event_tasks: id === "ep1" ? [taskEp1] : [taskEp2],
            });
        }),
    );
    mocked.replaceEventTasks.mockResolvedValue({ revision: 2 });
    render(<EventTaskEditor />);
    // 自动选中 ep1，请求 A 在途
    await screen.findByText("ep1");
    await waitFor(() => expect(resolvers["ep1"]).toBeDefined());
    // 切到 ep2，请求 B 在途（点 option 内部文本节点；点 wrapper 不触发提交）
    await user.click(screen.getByText("ep1"));
    const candidates = await screen.findAllByText("ep2");
    const inner =
      candidates.find((el) => el.classList.contains("ant-select-item-option-content")) ??
      candidates[candidates.length - 1];
    await user.click(inner);
    await waitFor(() => expect(resolvers["ep2"]).toBeDefined());
    // B 先回：显示 ep2
    await act(async () => {
      resolvers["ep2"]();
    });
    expect(await screen.findByText("任务 · task-ep2")).toBeTruthy();
    // A 后回：必须被忽略，UI 仍是 ep2
    await act(async () => {
      resolvers["ep1"]();
    });
    expect(screen.queryByText("任务 · task-ep1")).toBeNull();
    expect(screen.getByText("任务 · task-ep2")).toBeTruthy();
    // 保存必须写 ep2 的任务，而不是 A 的
    await user.click(screen.getByRole("button", { name: "保存订阅" }));
    await waitFor(() => expect(mocked.replaceEventTasks).toHaveBeenCalledTimes(1));
    const [endpointId, tasks] = mocked.replaceEventTasks.mock.calls[0] as [string, typeof GENERIC_TASK[]];
    expect(endpointId).toBe("ep2");
    expect(tasks.map((t) => t.id)).toEqual(["task-ep2"]);
  });
});
