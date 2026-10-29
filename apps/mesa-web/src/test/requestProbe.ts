// 请求记录探针（仅测试用，§4.4 / 第 3 步「请求顺序验证」）。
//
// 为什么需要单独一个探针：保存路径的**正确性**不只体现在「发了哪些请求」，还体现在
// 「按什么顺序发」。计数式断言（putCount === 1）无法证明：
//   - 运行中保存确实是 stop → PUT → 回读 GET → start；
//   - 回读不一致时没有出现第二个 PUT；
//   - 启动失败后的「重试」只发 start；
//   - Schema / 快照 / 未知状态这些守卫发生在任何写请求**之前**。
//
// 因此这里只做一件事：把每次 fetch 归成一个稳定标签并按顺序记录，测试直接断言
// 序列（或断言带顺序的「先于」关系），不依赖实现内部的调用次数统计。
import { vi } from "vitest";

/** 请求标签（稳定文案，便于在测试与验收报告里逐条对照）。 */
export type ProbeCall =
  | "GET:endpoints"
  | "GET:descriptor"
  /** I-2 定义目录（`/drivers/{id}/definitions`，可多页）。 */
  | "GET:definitions"
  /**
   * 对象探测（`POST /endpoints/{id}/browse`）。
   *
   * 单列一个标签而不是并入 OTHER：探测会临时拉起驱动进程读真机，"是否发了探测"
   * 与"按什么顺序发"本身就是验收条件（未连接端点必须零探测、探测早于预览）。
   */
  | "BROWSE"
  | "GET:tasks"
  | "PUT:tasks"
  | "STOP"
  | "START"
  | "PREVIEW"
  | "OTHER";

/**
 * 归类一次请求。
 *
 * 顺序刻意「先具体后一般」：`/api/v1/tasks?endpoint=` 是回读/快照读取，
 * `/api/v1/tasks/<id>` 才是写入，两者前缀相同但语义相反，不能混。
 */
export function classifyRequest(url: string, method?: string): ProbeCall {
  const m = (method ?? "GET").toUpperCase();
  if (url === "/api/v1/endpoints") return "GET:endpoints";
  if (url.startsWith("/api/v1/devices")) return "OTHER";
  if (url.endsWith("/definitions/identity-preview")) return "PREVIEW";
  // 目录分页请求与预览同前缀：预览已在上一条分流，这里只剩目录本体。
  if (m === "GET" && url.includes("/definitions")) return "GET:definitions";
  if (url.startsWith("/api/v1/drivers/")) return "GET:descriptor";
  if (m === "POST" && url.endsWith("/browse")) return "BROWSE";
  if (url.startsWith("/api/v1/tasks?endpoint=")) return "GET:tasks";
  if (m === "PUT" && url.startsWith("/api/v1/tasks/")) return "PUT:tasks";
  if (m === "POST" && url.endsWith("/stop")) return "STOP";
  if (m === "POST" && url.endsWith("/start")) return "START";
  return "OTHER";
}

export interface RequestProbe {
  /** 全部请求（按发生顺序）。 */
  calls: ProbeCall[];
  record: (url: string, method?: string) => ProbeCall;
  /** 清空记录（用于只观察「点击保存之后」的序列）。 */
  reset: () => void;
  /** 写路径序列：stop / PUT / 回读 GET / start（**保留顺序**，用于逐条比对）。 */
  writeSequence: () => ProbeCall[];
  /** 计数（便于断言「零 PUT」这类硬条件）。 */
  count: (call: ProbeCall) => number;
  /** 某个标签在序列中的首次下标（未出现返回 -1）。 */
  indexOf: (call: ProbeCall) => number;
}

/** 写路径相关标签：只有这四个参与「保存顺序」判定。 */
const WRITE_PATH: ProbeCall[] = ["STOP", "PUT:tasks", "GET:tasks", "START"];

export function createRequestProbe(): RequestProbe {
  const calls: ProbeCall[] = [];
  return {
    calls,
    record: (url, method) => {
      const call = classifyRequest(url, method);
      calls.push(call);
      return call;
    },
    reset: () => {
      calls.length = 0;
    },
    writeSequence: () => calls.filter((call) => WRITE_PATH.includes(call)),
    count: (call) => calls.filter((item) => item === call).length,
    indexOf: (call) => calls.indexOf(call),
  };
}

/** 把 fetch 桩挂到全局（返回同样被记录的 vi.fn，便于断言调用次数）。 */
export function installProbedFetch(
  probe: RequestProbe,
  handler: (url: string, init?: RequestInit) => Promise<unknown>,
) {
  const stub = vi.fn(async (url: string, init?: RequestInit) => {
    probe.record(String(url), init?.method);
    return handler(String(url), init);
  });
  (globalThis as { fetch?: unknown }).fetch = stub;
  return stub;
}
