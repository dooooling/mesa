// 浏览器后退守卫的**真实历史跳转**回归（§7-7 第 5 步）。
//
// 为什么另起一个文件：既有 `DraftNavGuard.test.tsx` 用 `MemoryRouter` + 手工
// `dispatchEvent(new PopStateEvent(...))`——那只能证明"弹框出现"，证明不了
// 「浏览器的历史条目真的动了以后，URL / 路由内容 / 草稿是否仍然一致」。
// 本文件用 **`BrowserRouter` + 真实 `window.history.back()`**（jsdom 会真的移动
// 条目并异步派发 popstate），因此覆盖的是用户实测的那条路径：
//
//   设备概览 → 采集 → 周期改 2000 → 后退 → 不选 → 再后退 → 点「继续编辑」
//   ⇒ 草稿仍在（2000）、地址与页面内容都还是采集页
//
// 断言口径（刻意不只看"弹框出现"）：
// 1. `window.location.pathname`（地址）与
// 2. 页面实际渲染的内容（采集页输入框 / 概览页"需要关注"标题）
// 3. 草稿值（周期输入框仍是 2000）
// 三者必须一致——旧缺陷正是"地址是采集页、内容是概览页"。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router-dom";
import { fireEvent } from "@testing-library/react";
import App from "../App";
import { installPointLiveStub, pointLiveRow } from "../test/pointLiveStub";

const EP = "ep-nav-1";
const DRIVER = "s7";
const DEVICE = "cnc-01";
const ACQ_PATH = `/devices/${DEVICE}/acquisition`;
const OVERVIEW_PATH = `/devices/${DEVICE}/overview`;

const REGISTER_RESOURCE = {
  id: "register",
  label: { default: "Register" },
  parameters: { fields: [] },
  outputs: [
    { id: "value", label: { default: "Value" }, type_spec: { kind: "fixed", data_type: "F64" }, access: "read" },
  ],
  modes: ["poll"],
};

const ENDPOINTS = [{ id: EP, name: "采集连接 A", driver_id: DRIVER, device_id: DEVICE, state: "STOPPED" }];

/** 一个已保存的 poll 任务（周期 1000）：有它才有「已保存周期」可比对。 */
const TASKS = [
  {
    id: "t1",
    schedule: { mode: "poll", interval_ms: 1000 },
    binding: {
      kind: "mesa.resources.v1",
      config: {
        selections: [
          { resource_id: "register", parameters: {}, outputs: [{ output: "value", point_key: "register.1.value" }] },
        ],
      },
    },
  },
];

const DESCRIPTOR = {
  contract_major: 2,
  contract_minor: 1,
  identity: { driver_id: DRIVER, name: "S7", version: "0.0.1" },
  connection: { fields: [] },
  resources: [REGISTER_RESOURCE],
  controls: { commands: [] },
  resource_selection_methods: ["manual"],
  capabilities: { poll: true, subscribe: false, write: false, method: false, events: false },
  events: { streams: [] },
};

/** 每次真实历史跳转落到的 pathname（含我们主动中和的那几次）。 */
let pops: string[] = [];
const recordPop = () => {
  pops.push(window.location.pathname);
};

/**
 * 挂载：历史条目先摆成「设备概览 → 采集」两页，再渲染 `BrowserRouter` 版 App。
 *
 * 不用 `MemoryRouter`：MemoryRouter 根本不写 `window.history`，`history.back()`
 * 不会移动任何条目，也就无从验证"URL 与内容是否一致"。
 */
function mount() {
  window.history.replaceState(null, "", OVERVIEW_PATH);
  window.history.pushState(null, "", ACQ_PATH);
  installPointLiveStub({
    points: [pointLiveRow(EP, "register.1.value", "GOOD", 500, 1)],
    endpoints: ENDPOINTS,
    descriptor: DESCRIPTOR,
    tasks: TASKS,
    device: { id: DEVICE, name: "CNC-01" },
  });
  return render(
    <BrowserRouter>
      <App />
    </BrowserRouter>,
  );
}

function intervalValue(): string {
  return (screen.getByTestId("acquisition-interval-input") as HTMLInputElement).value;
}

function leaveDialog(): HTMLElement | null {
  const nodes = document.querySelectorAll(".ant-modal-confirm");
  return nodes.length ? (nodes[nodes.length - 1] as HTMLElement) : null;
}

/** 等采集窗格就绪（周期输入出现）并把周期改成 2000（制造未保存草稿）。 */
async function dirtyInterval() {
  const interval = (await screen.findByTestId("acquisition-interval-input")) as HTMLInputElement;
  fireEvent.change(interval, { target: { value: "2000" } });
  await screen.findByTestId("draft-save-bar");
  await waitFor(() => expect(intervalValue()).toBe("2000"));
}

/** 等确认框出现（真实 popstate 是异步的：必须 waitFor，不能同步断言）。 */
async function expectDialog(): Promise<HTMLElement> {
  return waitFor(() => {
    const modal = leaveDialog();
    expect(modal).not.toBeNull();
    return modal!;
  });
}

async function chooseStay(user: ReturnType<typeof userEvent.setup>) {
  const modal = leaveDialog();
  if (!modal) throw new Error("确认框未出现");
  await user.click(within(modal).getByRole("button", { name: "继续编辑" }));
  await waitFor(() => expect(leaveDialog()).toBeNull());
}

async function chooseLeave(user: ReturnType<typeof userEvent.setup>) {
  const modal = leaveDialog();
  if (!modal) throw new Error("确认框未出现");
  await user.click(within(modal).getByRole("button", { name: "放弃修改并继续" }));
}

/** 页面内容确实是采集页（不是"地址是采集页、内容是概览页"）。 */
function expectAcquisitionContent() {
  expect(screen.getByTestId("acquisition-interval-input")).toBeTruthy();
  expect(screen.queryByText("需要关注")).toBeNull();
}

/** 页面内容确实是设备概览。 */
function expectOverviewContent() {
  expect(screen.queryByTestId("acquisition-interval-input")).toBeNull();
  expect(screen.getByText("需要关注")).toBeTruthy();
}

beforeEach(() => {
  vi.clearAllMocks();
  pops = [];
  // 记录器先于 App 挂载（也就先于路由/守卫的监听器）：它看到的是**跳转落地那一刻**
  // 的地址，因此能证明"每一次后退都被真实处理过"。
  window.addEventListener("popstate", recordPop);
  window.history.replaceState(null, "", "/");
});

afterEach(() => {
  window.removeEventListener("popstate", recordPop);
  vi.unstubAllGlobals();
});

describe("浏览器后退（真实历史跳转）：不丢草稿、地址与内容一致", () => {
  it("real_back_asks_and_keeps_draft_when_cancelled", async () => {
    const user = userEvent.setup();
    mount();
    await dirtyInterval();

    window.history.back();
    await expectDialog();
    // 后退确实发生了（不是"什么都没动"），但落点仍是被拦住的采集页条目。
    expect(pops).toEqual([ACQ_PATH]);

    await chooseStay(user);

    // 取消 ⇒ 地址仍是采集页，内容也是采集页，草稿仍是 2000。
    expect(window.location.pathname).toBe(ACQ_PATH);
    expectAcquisitionContent();
    expect(intervalValue()).toBe("2000");

    // 取消后再后退一次：守卫仍然有效（哨兵已补回），不得因为"刚取消过"就放行。
    window.history.back();
    await expectDialog();
    await chooseStay(user);
    expect(window.location.pathname).toBe(ACQ_PATH);
    expectAcquisitionContent();
    expect(intervalValue()).toBe("2000");
  }, 60_000);

  it("every_back_during_pending_confirmation_is_neutralised", async () => {
    const user = userEvent.setup();
    mount();
    await dirtyInterval();

    window.history.back();
    await expectDialog();

    // 待确认期间**连续后退三次**：旧实现只拦得住第一次（第二次 URL 一变就直接放行），
    // 路由切到概览、采集页卸载、草稿丢失。
    window.history.back();
    await waitFor(() => expect(pops).toHaveLength(2));
    window.history.back();
    await waitFor(() => expect(pops).toHaveLength(3));

    // 每一次都被逐个中和：三次真实历史跳转全部落在采集页 URL 上。
    expect(pops).toEqual([ACQ_PATH, ACQ_PATH, ACQ_PATH]);
    expect(window.location.pathname).toBe(ACQ_PATH);
    expectAcquisitionContent();
    // 且不会叠出多个确认框。
    expect(document.querySelectorAll(".ant-modal-confirm")).toHaveLength(1);

    await chooseStay(user);
    expect(window.location.pathname).toBe(ACQ_PATH);
    expectAcquisitionContent();
    expect(intervalValue()).toBe("2000");
  }, 60_000);

  it("user_reported_sequence_back_twice_then_continue_editing_keeps_draft", async () => {
    const user = userEvent.setup();
    mount();
    await dirtyInterval();

    // 用户实测顺序：后退 → 不选 → 再后退 → 点「继续编辑」。
    window.history.back();
    const dialog = await expectDialog();
    expect(dialog.textContent ?? "").toContain("离开采集页会丢弃当前未保存的采集配置修改");

    window.history.back();
    await waitFor(() => expect(pops).toHaveLength(2));

    await chooseStay(user);

    // 三处一致：地址、页面内容、草稿。
    expect(window.location.pathname).toBe(ACQ_PATH);
    expectAcquisitionContent();
    expect(intervalValue()).toBe("2000");
    // 草稿仍是"未保存"状态（保存区还在，说明不是靠"重新加载"骗过断言）。
    expect(screen.getByTestId("draft-save-bar")).toBeTruthy();
  }, 60_000);

  it("confirmed_leave_navigates_exactly_once_to_the_previous_page", async () => {
    const user = userEvent.setup();
    mount();
    await dirtyInterval();

    window.history.back();
    await expectDialog();
    await chooseLeave(user);

    await waitFor(() => expect(window.location.pathname).toBe(OVERVIEW_PATH));
    // 只跳一次：后退落地（采集页条目）+ 确认后的一次导航（概览），没有多跳/漏跳。
    expect(pops).toEqual([ACQ_PATH, OVERVIEW_PATH]);
    expectOverviewContent();
    expect(window.location.pathname).toBe(OVERVIEW_PATH);
  }, 60_000);

  it("cross_step_traversal_is_neutralised_immediately_and_keeps_draft", async () => {
    const user = userEvent.setup();
    mount();
    await dirtyInterval();

    window.history.back();
    await expectDialog();

    // 跨步遍历：真实浏览器里"连点两次后退"可能被合并成一次多步跳转；jsdom 会把
    // 同 tick 的多次 back() 折叠，因此这里用 go(-2) 精确驱动同一段代码
    // （一次就跨过了采集页条目，URL 真的变成了概览页）。
    window.history.go(-2);
    await waitFor(() => expect(pops).toEqual([ACQ_PATH, OVERVIEW_PATH]));

    // 立即中和：地址与内容都被拉回采集页（不会停在"地址是概览页"上）。
    await waitFor(() => expect(window.location.pathname).toBe(ACQ_PATH));
    expectAcquisitionContent();
    // 草稿仍在：即使中间态真的把采集页卸载过，副本（路由之外）也会恢复它。
    await waitFor(() => expect(intervalValue()).toBe("2000"));
    expect(screen.getByTestId("draft-save-bar")).toBeTruthy();

    await chooseStay(user);
    expect(window.location.pathname).toBe(ACQ_PATH);
    expectAcquisitionContent();
    expect(intervalValue()).toBe("2000");
  }, 60_000);

  it("confirmed_leave_after_cross_step_traversal_goes_to_the_recorded_target", async () => {
    const user = userEvent.setup();
    mount();
    await dirtyInterval();

    window.history.back();
    await expectDialog();
    window.history.go(-2);
    await waitFor(() => expect(window.location.pathname).toBe(ACQ_PATH));

    await chooseLeave(user);

    // 用户原本想去的位置被记下来了：确认后一次导航就落到那里。
    await waitFor(() => expect(window.location.pathname).toBe(OVERVIEW_PATH));
    expectOverviewContent();
  }, 60_000);
});
