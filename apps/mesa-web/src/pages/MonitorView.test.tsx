// PR31 Gate（P1-1）：Monitor STALE 在 API 故障时不得冻结。
// points 轮询失败无 setState、无 rerender——STALE 判定用的 now 必须由独立
// 时钟推进，否则 age 停在最后一次成功时刻，故障时永远不出现 STALE。
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, render, screen } from "@testing-library/react";
import { MonitorView } from "./MonitorView";
import { POINT_STALE_AFTER_MS } from "../deviceModel";

function mockPoints(handler: () => Promise<{ ok: boolean; status: number; body: unknown }>) {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
    if (url === "/api/v1/devices") return { ok: true, status: 200, json: async () => ({ devices: [] }) };
    if (url === "/api/v1/endpoints") return { ok: true, status: 200, json: async () => ({ endpoints: [] }) };
    const r = await handler();
    return { ok: r.ok, status: r.status, json: async () => r.body };
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("MonitorView STALE", () => {
  it("points 持续失败时，已有点位的 age 照常推进并出现 STALE", async () => {
    const t0 = Date.now();
    // 首轮成功：一个 5s 前的点；随后 points API 持续失败
    let fail = false;
    mockPoints(async () => {
      if (!fail) {
        return {
          ok: true,
          status: 200,
          body: {
            points: [
              {
                endpoint_id: "ep1",
                key: "k1",
                point_id: 1,
                quality: "GOOD",
                type: "f64",
                value: 42,
                timestamp_ns: (t0 - 5000) * 1e6,
              },
            ],
          },
        };
      }
      return { ok: false, status: 500, body: { error: { message: "boom" } } };
    });
    render(<MonitorView />);
    // 首屏有点、无 STALE（5s < 30s）
    expect(await screen.findByText("k1")).toBeTruthy();
    expect(screen.queryByText("STALE")).toBeNull();
    // 后续持续失败：不真实跑 30 个 interval（逐个触发 rerender + fetch，
    // CI 上跑到 5.6s 被 5s wall 截掉）；直接把 wall clock 推到 STALE 阈值
    // 之后，再只触发一个 1s tick，验证失败拉取保留 snapshot + 独立时钟推进。
    fail = true;
    vi.setSystemTime(t0 + POINT_STALE_AFTER_MS + 1000);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000);
    });
    // k1 不消失（失败保留）且同时出现 STALE（时钟推进）
    expect(screen.getByText("k1")).toBeTruthy();
    expect(screen.getByText("STALE")).toBeTruthy();
  });
});
