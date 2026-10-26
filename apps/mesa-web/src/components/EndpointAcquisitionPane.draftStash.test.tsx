// 草稿副本（`draftStash`）回归：草稿内容必须在**路由之外**活下来（§7-7 第 5 步）。
//
// 为什么必须有这条：声明式路由（`BrowserRouter` + `Routes`）下 `popstate` 时路由
// 已经变了，采集页会真的卸载——组件内 `useState` 的草稿随卸载消失，事后无法"取消"。
// 因此草稿内容另有副本，重新挂载后恢复成**未保存草稿**。
//
// 本文件用**真实卸载再挂载**（换 key 强制重建）验证这一点，不依赖浏览器历史语义，
// 所以它在 jsdom 里就是可自动验证的强断言：
// 1. 有未保存修改时卸载 → 重新挂载后周期仍是 2000，且保存区仍在（= 仍是草稿）；
// 2. 没有未保存修改时卸载 → 重新挂载后是服务端值 1000，且不出现保存区（不留残留）。
import { describe, expect, it, vi, beforeEach } from "vitest";
import { useState } from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { EndpointAcquisitionPane } from "./EndpointAcquisitionPane";
import { DraftStashProvider } from "../draftStash";

const EP = "ep-stash";
const DRIVER = "drv";

const RESOURCES = [
  {
    id: "register",
    label: { default: "Register" },
    parameters: { fields: [] },
    outputs: [
      { id: "value", label: { default: "Value" }, type_spec: { kind: "fixed", data_type: "F64" }, access: "read" },
    ],
    modes: ["poll"],
  },
];

const TASKS = {
  tasks: [
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
  ],
};

function installFetch() {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
    if (url.startsWith("/api/v1/drivers/")) {
      return { ok: true, status: 200, json: async () => ({ resources: RESOURCES }) };
    }
    if (url.startsWith("/api/v1/tasks?endpoint=")) {
      return { ok: true, status: 200, json: async () => TASKS };
    }
    if (url === "/api/v1/endpoints") {
      return {
        ok: true,
        status: 200,
        json: async () => ({ endpoints: [{ id: EP, driver_id: DRIVER, runtime: { state: "STOPPED" } }] }),
      };
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
}

/** 卸载/重挂载只有一个入口：换 key 强制重建采集窗格（模拟路由把它卸载再挂回来）。 */
function Harness({ onDraft }: { onDraft?: (state: { dirty: boolean; saving: boolean }) => void }) {
  const [nonce, setNonce] = useState(0);
  return (
    <div>
      <button type="button" data-testid="remount-pane" onClick={() => setNonce((n) => n + 1)}>
        重挂载
      </button>
      <DraftStashProvider>
        <EndpointAcquisitionPane
          key={nonce}
          endpointId={EP}
          driverId={DRIVER}
          onChanged={() => {}}
          onDraftStateChange={onDraft}
        />
      </DraftStashProvider>
    </div>
  );
}

async function ready() {
  await waitFor(() => expect(screen.getByTestId("acquisition-interval-input")).toBeTruthy());
  await waitFor(() => expect(screen.queryByTestId("tasks-load-error")).toBeNull());
}

function intervalValue(): string {
  return (screen.getByTestId("acquisition-interval-input") as HTMLInputElement).value;
}

beforeEach(() => {
  vi.clearAllMocks();
  installFetch();
});

describe("草稿副本：卸载后草稿不丢、无修改时不留残留", () => {
  it("unsaved_draft_survives_a_real_unmount_and_is_restored_as_draft", async () => {
    const user = userEvent.setup();
    const reports: Array<{ dirty: boolean; saving: boolean }> = [];
    render(<Harness onDraft={(state) => reports.push(state)} />);
    await ready();
    expect(intervalValue()).toBe("1000");

    fireEvent.change(screen.getByTestId("acquisition-interval-input"), { target: { value: "2000" } });
    await screen.findByTestId("draft-save-bar");
    await waitFor(() => expect(intervalValue()).toBe("2000"));

    // 真实卸载（等价于路由变化把采集页卸载掉）。
    await user.click(screen.getByTestId("remount-pane"));
    await ready();

    // 恢复成**草稿**：周期仍是 2000，且保存区仍在（不是"看起来有值但服务端已生效"）。
    await waitFor(() => expect(intervalValue()).toBe("2000"));
    expect(screen.getByTestId("draft-save-bar")).toBeTruthy();
    // 并且重新上报了 dirty（外壳守卫据此继续拦截离开）。
    await waitFor(() => expect(reports[reports.length - 1]).toEqual({ dirty: true, saving: false }));
  }, 60_000);

  it("clean_pane_does_not_resurrect_a_stale_draft_after_remount", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await ready();
    expect(intervalValue()).toBe("1000");

    await user.click(screen.getByTestId("remount-pane"));
    await ready();

    // 没有未保存修改 ⇒ 不留副本：重挂载后是服务端值，保存区不出现。
    await waitFor(() => expect(intervalValue()).toBe("1000"));
    expect(screen.queryByTestId("draft-save-bar")).toBeNull();
  }, 60_000);
});
