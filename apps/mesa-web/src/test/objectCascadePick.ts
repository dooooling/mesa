// 级联下拉的测试助手：**驱动夹具** + 逐级选择动作（仅测试用）。
//
// 为什么集中成一个助手：
// - 树 id、节点形状、选择载荷、诊断码**只能来自驱动产出的契约夹具**
//   （`nckIdentityProbeContract.json`，由
//   `drivers/sinumerik-nck/tests/identity_probe_trees.rs` 真实浏览后导出），
//   测试里不得手写 `nck://…` 字符串，否则会出现"前端测试全绿、真机走不通"；
// - 夹具只导出前端**原流程**下钻的那几页（根 / presets / identity / channels /
//   spindles）。联动扫描会遍历全部目录分支，未导出的分支（axes / objects）在
//   测试桩里如实返回驱动的 `UNKNOWN_PARENT` 错误——这不是"设备上没有对象"，
//   而是"探测不完整"，正好用来钉住两种口径的差别；
// - 逐级选择的交互（展开下拉 → 探测 → 选实际对象）在 9 个测试文件里重复，
//   集中一处才能保证定位口径唯一。
import { fireEvent, screen, waitFor } from "@testing-library/react";
import { expect } from "vitest";
import contract from "./nckIdentityProbeContract.json";

type ContractNode = Record<string, unknown>;
export interface ContractPage {
  nodes: ContractNode[];
  next_cursor: string | null;
}

const CONTRACT = contract as unknown as { family: string; pages: Record<string, ContractPage> };

/** 驱动契约里的真实 parent（不手写字符串，全部从夹具 key 取）。 */
export const PROBE_ROOT = "";
export const PROBE_IDENTITY = "nck://identity";
export const PROBE_CHANNELS = "nck://identity/channels";
export const PROBE_SPINDLES = "nck://identity/spindles";

/** 某一 parent 的真实响应体；夹具没导出的分支返回 null（调用方按驱动行为报错）。 */
export function probePageOf(parent: string): ContractPage | null {
  return CONTRACT.pages[parent] ?? null;
}

/** 夹具里导出的全部 parent（断言"扫到了哪些分支"时用）。 */
export function probeParents(): string[] {
  return Object.keys(CONTRACT.pages);
}

/** 按标签关键字在某一页找节点（找不到即失败：说明驱动树变了，必须重生成夹具）。 */
export function probeNodeByLabel(parent: string, keyword: string): ContractNode {
  const page = probePageOf(parent);
  const node = page?.nodes.find((candidate) =>
    String(candidate.label ?? "").includes(keyword),
  );
  if (!node) {
    throw new Error(
      `驱动契约里没有「${keyword}」节点（parent=${parent}）：请重生成 nckIdentityProbeContract.json`,
    );
  }
  return node;
}

/** 通道实例节点（实例号在标签里，id 来自夹具）。 */
export function channelNode(n: number): ContractNode {
  const node = probePageOf(PROBE_CHANNELS)?.nodes.find((candidate) =>
    String(candidate.label ?? "").startsWith(`通道${n}`),
  );
  if (!node) throw new Error(`驱动契约里没有通道${n}`);
  return node;
}

/** 主轴实例节点。 */
export function spindleNode(n: number): ContractNode {
  const node = probePageOf(PROBE_SPINDLES)?.nodes.find((candidate) =>
    String(candidate.label ?? "").startsWith(`主轴${n}`),
  );
  if (!node) throw new Error(`驱动契约里没有主轴${n}`);
  return node;
}

/**
 * 探测响应（与 Core REST 同形）。
 *
 * - 夹具导出的 parent ⇒ 200 + 真实节点；
 * - 未导出的 parent ⇒ 400 `UNKNOWN_PARENT`（驱动的真实行为：未知父节点是错误，
 *   不是空页）。联动扫描把它记成"分支不可达"，绝不当成"设备上没有对象"。
 */
export function probeResponse(
  parent: string,
):
  | { ok: true; status: 200; json: () => Promise<unknown> }
  | { ok: false; status: 400; json: () => Promise<unknown> } {
  const page = probePageOf(parent);
  if (!page) {
    return {
      ok: false,
      status: 400,
      json: async () => ({
        error: { code: "UNKNOWN_PARENT", message: `未知的浏览父节点 \`${parent}\`` },
      }),
    };
  }
  return { ok: true, status: 200, json: async () => page };
}

/** 空页响应（"探测成功但没有对象"的口径）。 */
export function emptyProbeResponse() {
  return { ok: true, status: 200, json: async () => ({ nodes: [], next_cursor: null }) };
}

/** 当前某一级下拉里可选的选项（排除占位项与 disabled 的不可用候选）。 */
export function cascadeOptions(key: string): HTMLOptionElement[] {
  const select = screen.getByTestId(`nck-object-select-${key}`) as HTMLSelectElement;
  return Array.from(select.querySelectorAll("option")).filter(
    (option) => option.value !== "" && !option.disabled,
  );
}

/**
 * 逐级选择：展开下拉（触发按需探测）→ 等候选 → 按标签关键字选一个实际对象。
 *
 * 断言口径与用户操作一致：先看到候选，才能选；选完该级变成"已选"标签。
 */
export async function pickObject(key: string, keyword: string): Promise<string> {
  const select = screen.getByTestId(`nck-object-select-${key}`) as HTMLSelectElement;
  fireEvent.focus(select);
  await waitFor(() => expect(cascadeOptions(key).length).toBeGreaterThan(0));
  const option = cascadeOptions(key).find((item) =>
    String(item.textContent ?? "").includes(keyword),
  );
  if (!option) {
    const visible = cascadeOptions(key).map((item) => item.textContent);
    throw new Error(`下拉「${key}」里找不到含「${keyword}」的候选（当前：${visible.join(" | ")}）`);
  }
  fireEvent.change(select, { target: { value: option.value } });
  await waitFor(() =>
    expect(screen.getByTestId(`nck-object-selected-${key}`).getAttribute("data-value")).not.toBe(
      null,
    ),
  );
  return option.value;
}

/** 只展开下拉（触发探测）而不选择：用于"探测失败/无候选"类断言。 */
export async function openCascade(key: string): Promise<HTMLSelectElement> {
  const select = screen.getByTestId(`nck-object-select-${key}`) as HTMLSelectElement;
  fireEvent.focus(select);
  return select;
}
