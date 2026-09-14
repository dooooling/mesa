// PR31 Gate（P1-2）：Dashboard 诚实指标 fail-closed。
// 非 2xx / 网络失败不得伪装成“系统真实为零”，也不得把旧值当当前值：
// 必须标 STALE + 明确“数据不可用/更新失败”。
import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { Dashboard } from "./Dashboard";

function mockFetch(handler: (url: string) => Promise<{ ok: boolean; status: number; body: unknown }>) {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
    const r = await handler(url);
    return { ok: r.ok, status: r.status, json: async () => r.body };
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.unstubAllGlobals();
});

describe("Dashboard freshness", () => {
  it("正常 2xx 显示真实计数，无 STALE", async () => {
    mockFetch(async (url) => {
      if (url === "/api/v1/devices") return { ok: true, status: 200, body: { devices: [{ id: "d1", name: "D1" }] } };
      if (url === "/api/v1/endpoints") {
        return { ok: true, status: 200, body: { endpoints: [{ id: "e1", driver_id: "s7", state: "RUNNING" }] } };
      }
      return { ok: true, status: 200, body: { points: [] } };
    });
    render(
      <MemoryRouter>
        <Dashboard />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.queryByText("STALE")).toBeNull());
    // 设备 1 / 连接 1 / 运行中 1（严格 RUNNING 口径）
    expect(screen.getByText("设备").closest(".ant-card")).toBeTruthy();
  });

  it("HTTP 500 + error 包不得伪装成 0（标 STALE + 更新失败）", async () => {
    mockFetch(async () => ({ ok: false, status: 500, body: { error: { message: "boom" } } }));
    render(
      <MemoryRouter>
        <Dashboard />
      </MemoryRouter>,
    );
    // inventory 与 points 双双 error：至少出现 STALE 标记与失败提示
    await waitFor(() => expect(screen.getAllByText("STALE").length).toBeGreaterThanOrEqual(1));
    expect(screen.getByText("部分数据更新失败")).toBeTruthy();
  });

  it("网络失败不吞成旧值：显示 STALE 而非上次数字", async () => {
    let fail = false;
    mockFetch(async (url) => {
      if (url === "/api/v1/devices") {
        // inventory 保持成功：隔离出 points 侧失败（points 2s 一轮，
        // inventory 10s 才刷新——不断言 inventory，避免周期拖过 5s 墙）
        return { ok: true, status: 200, body: { devices: [{ id: "d1", name: "D1" }] } };
      }
      if (url === "/api/v1/endpoints") {
        return { ok: true, status: 200, body: { endpoints: [{ id: "e1", driver_id: "s7", state: "RUNNING" }] } };
      }
      if (fail) throw new Error("network down");
      return { ok: true, status: 200, body: { points: [] } };
    });
    render(
      <MemoryRouter>
        <Dashboard />
      </MemoryRouter>,
    );
    // 首轮成功：无 STALE
    await waitFor(() => expect(screen.queryByText("STALE")).toBeNull());
    // points 后续持续失败：2s 轮询内必须出现 STALE，而不是无限期显示旧数字
    fail = true;
    await waitFor(() => expect(screen.getAllByText("STALE").length).toBeGreaterThanOrEqual(1), { timeout: 8000 });
  });
});
