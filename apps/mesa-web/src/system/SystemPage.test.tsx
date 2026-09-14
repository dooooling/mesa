// M4.3 回归：System 页只展示真实可确认状态。
// - Core 版本/运行时长/存储计数来自 /diagnostics；
// - Drivers 来自 /drivers 清单，状态只写 AVAILABLE（已发现），不写 HEALTHY；
// - 事件服务可用即 AVAILABLE，503 即 UNAVAILABLE，其它失败明示原因。
import { describe, expect, it, vi, afterEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import App from "../App";

function mockSystem(opts: { failDiag?: boolean; failDrivers?: boolean; events503?: boolean }) {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
    if (url === "/api/v1/diagnostics") {
      if (opts.failDiag) return { ok: false, status: 500, json: async () => ({}) };
      return {
        ok: true,
        status: 200,
        json: async () => ({
          version: "2.1.0",
          uptime_secs: 3700,
          devices: { stored: 2 },
          endpoints: { stored: 3, runtime: 3 },
        }),
      };
    }
    if (url === "/api/v1/drivers") {
      if (opts.failDrivers) return { ok: false, status: 500, json: async () => ({}) };
      return {
        ok: true,
        status: 200,
        json: async () => ({ drivers: [{ id: "simulator", name: "Simulator", version: "0.3.0" }] }),
      };
    }
    if (url === "/api/v1/events/stats") {
      if (opts.events503) {
        return {
          ok: false,
          status: 503,
          json: async () => ({ error: { code: "EVENT_STORE_UNAVAILABLE", message: "down" } }),
        };
      }
      return { ok: true, status: 200, json: async () => ({ stored_rows: 10, live_clients: 1 }) };
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("M4.3 SystemPage", () => {
  it("展示真实状态：版本/驱动 AVAILABLE/事件 AVAILABLE", async () => {
    mockSystem({});
    render(
      <MemoryRouter initialEntries={["/system"]}>
        <App />
      </MemoryRouter>,
    );
    await screen.findByText("2.1.0");
    expect(screen.getByText("simulator")).toBeTruthy();
    expect(screen.getAllByText("AVAILABLE").length).toBeGreaterThanOrEqual(2);
    // 不写 HEALTHY（API 证明不了健康，只证明已发现/可用）
    expect(screen.queryByText("HEALTHY")).toBeNull();
  });

  it("事件 503 时标 UNAVAILABLE，不写正常", async () => {
    mockSystem({ events503: true });
    render(
      <MemoryRouter initialEntries={["/system"]}>
        <App />
      </MemoryRouter>,
    );
    await screen.findByText("Event service unavailable");
  });

  it("诊断失败时明确不可用，不编造数字", async () => {
    mockSystem({ failDiag: true });
    render(
      <MemoryRouter initialEntries={["/system"]}>
        <App />
      </MemoryRouter>,
    );
    await screen.findByText("核心诊断不可用");
  });
});
