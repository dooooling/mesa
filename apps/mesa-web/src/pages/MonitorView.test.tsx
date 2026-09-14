// PR31 Gate（P1-1）：Monitor STALE 在 API 故障时不得冻结。
// points 轮询失败无 setState、无 rerender——STALE 判定用的 now 必须由独立
// 时钟推进，否则 age 停在最后一次成功时刻，故障时永远不出现 STALE。
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
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
    // 后续持续失败：时钟照常推进，超过 30s 后必须出现 STALE
    fail = true;
    await vi.advanceTimersByTimeAsync(POINT_STALE_AFTER_MS);
    await waitFor(() => expect(screen.getByText("STALE")).toBeTruthy());
  });
});
