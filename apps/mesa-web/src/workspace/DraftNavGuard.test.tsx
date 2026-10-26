// 应用内离开导航守卫的回归测试（用户实测路径，§7-7）。
//
// 现场复现：改采集周期为 2000 → 点左侧「总览」→ **没有确认** → 回到采集页周期变回
// 1000（草稿被静默丢弃）。修复后四条应用内离开路径（侧栏 / 面包屑 / 设备 Tab /
// 浏览器前进后退）必须共用**同一个**守卫：有草稿先确认，取消则本次导航不发生，
// 且**草稿内容仍在**（本文件逐条断言周期输入框仍是 2000，而不是只看确认框出现）。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import App from "../App";
import { installPointLiveStub, pointLiveRow } from "../test/pointLiveStub";

const EP = "ep-nav-1";
const DRIVER = "s7";

const REGISTER_RESOURCE = {
  id: "register",
  label: { default: "Register" },
  parameters: { fields: [] },
  outputs: [
    { id: "value", label: { default: "Value" }, type_spec: { kind: "fixed", data_type: "F64" }, access: "read" },
  ],
  modes: ["poll"],
};

const ENDPOINTS = [{ id: EP, name: "采集连接 A", driver_id: DRIVER, device_id: "cnc-01", state: "STOPPED" }];

/** 一个已保存的 poll 任务（周期 1000）：有它才有「已保存周期」可比对，改周期才算变更。 */
const TASKS = [
  {
    id: "t1",
    schedule: { mode: "poll", interval_ms: 1000 },
    binding: {
      kind: "mesa.resources.v1",
      config: {
        selections: [
          {
            resource_id: "register",
            parameters: {},
            outputs: [{ output: "value", point_key: "register.1.value" }],
          },
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
  // 不支持事件：事件订阅区域不应出现（本条由 DeviceAcquisition.eventsRegion 覆盖）。
  events: { streams: [] },
};

function mount() {
  installPointLiveStub({
    points: [pointLiveRow(EP, "register.1.value", "GOOD", 500, 1)],
    endpoints: ENDPOINTS,
    descriptor: DESCRIPTOR,
    tasks: TASKS,
  });
  return render(
    <MemoryRouter initialEntries={[`/devices/cnc-01/acquisition`]}>
      <App />
    </MemoryRouter>,
  );
}

/** 等采集窗格就绪（周期输入出现）并把周期改成 2000（制造未保存草稿）。 */
async function dirtyInterval(user: ReturnType<typeof userEvent.setup>) {
  const interval = (await screen.findByTestId("acquisition-interval-input")) as HTMLInputElement;
  fireEvent.change(interval, { target: { value: "2000" } });
  await screen.findByTestId("draft-save-bar");
  await waitFor(() => expect(intervalValue()).toBe("2000"));
  return user;
}

function intervalValue(): string {
  return (screen.getByTestId("acquisition-interval-input") as HTMLInputElement).value;
}

function leaveDialog(): HTMLElement | null {
  const nodes = document.querySelectorAll(".ant-modal-confirm");
  return nodes.length ? (nodes[nodes.length - 1] as HTMLElement) : null;
}

/** 断言出现「离开采集页会丢弃未保存修改」的确认框，并选「继续编辑」（取消本次导航）。 */
async function expectDialogThenStay(user: ReturnType<typeof userEvent.setup>) {
  const modal = await waitFor(() => {
    const el = leaveDialog();
    expect(el).not.toBeNull();
    return el!;
  });
  expect(modal.textContent ?? "").toContain("离开采集页会丢弃当前未保存的采集配置修改，确定继续？");
  await user.click(within(modal).getByRole("button", { name: "继续编辑" }));
  await waitFor(() => expect(leaveDialog()).toBeNull());
  // 取消 ⇒ 本次导航不发生，且**草稿内容仍在**（不是只看确认框出现过）。
  await waitFor(() => expect(intervalValue()).toBe("2000"));
}

/** 侧栏项（Menu 渲染为 li，面包屑是 <a>）。 */
function sidebarItem(label: string): HTMLElement {
  const li = screen
    .getAllByText(label)
    .find((el) => el.closest(".ant-menu-item") !== null || el.classList.contains("ant-menu-title-content"));
  if (!li) throw new Error(`侧栏找不到 ${label}`);
  return li;
}

/** 面包屑「设备」（跳过侧栏同名项）。 */
function breadcrumbDevice(): HTMLElement {
  const link = screen.getAllByText("设备").find((el) => el.tagName.toLowerCase() === "a");
  if (!link) throw new Error("面包屑找不到「设备」");
  return link;
}

function tabLink(label: string): HTMLElement {
  return within(screen.getByTestId("device-tab-nav")).getByText(label);
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("应用内离开路径统一守卫（侧栏 / 面包屑 / Tab / 前进后退）", () => {
  it("sidebar_navigation_asks_and_keeps_the_draft_when_cancelled", async () => {
    const user = userEvent.setup();
    mount();
    await dirtyInterval(user);

    await user.click(sidebarItem("总览"));
    await expectDialogThenStay(user);
    // 仍停在采集页（没有静默跳走）。
    expect(screen.getByTestId("acquisition-interval-input")).toBeTruthy();
  }, 60_000);

  it("breadcrumb_navigation_asks_and_keeps_the_draft_when_cancelled", async () => {
    const user = userEvent.setup();
    mount();
    await dirtyInterval(user);

    await user.click(breadcrumbDevice());
    await expectDialogThenStay(user);
    expect(screen.getByTestId("acquisition-interval-input")).toBeTruthy();
  }, 60_000);

  it("device_tab_navigation_uses_the_same_guard", async () => {
    const user = userEvent.setup();
    mount();
    await dirtyInterval(user);

    await user.click(tabLink("实时数据"));
    await expectDialogThenStay(user);
    expect(screen.getByTestId("acquisition-interval-input")).toBeTruthy();
  }, 60_000);

  it("browser_back_popstate_asks_and_keeps_the_draft_when_cancelled", async () => {
    const user = userEvent.setup();
    mount();
    await dirtyInterval(user);

    // 浏览器后退：守卫用「同 URL 哨兵条目」把 popstate 变成一次确认机会。
    act(() => {
      window.dispatchEvent(new PopStateEvent("popstate"));
    });
    await expectDialogThenStay(user);
    expect(screen.getByTestId("acquisition-interval-input")).toBeTruthy();
  }, 60_000);

  it("navigation_without_draft_does_not_ask", async () => {
    const user = userEvent.setup();
    mount();
    await screen.findByTestId("acquisition-interval-input");
    // 无修改：不弹框，直接离开。
    await user.click(sidebarItem("总览"));
    await waitFor(() => expect(screen.queryByTestId("acquisition-interval-input")).toBeNull());
    expect(leaveDialog()).toBeNull();
  }, 60_000);
});
