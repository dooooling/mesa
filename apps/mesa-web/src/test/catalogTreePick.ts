// 目录树选变量助手（仅测试用）。
//
// 为什么需要它：评审要求移除与分类树重复的**平铺变量下拉框**（`nck-variable-select`），
// 变量选择入口只剩「搜索框 + 分类树」。于是所有原本 `user.selectOptions(select, id)`
// 的用例都必须改成「在树上点叶子」，而这件事散落在 8 个测试文件里——集中成一个助手
// 才能保证：
//   - 叶子定位口径唯一（`catalog-tree-leaf-<variableId>`，与 `CatalogCategoryTree` 的
//     约定同源，不在测试里各写各的选择器）；
//   - 分组折叠的兜底逻辑只写一遍（树默认全展开，但用例可以手动折叠分组）；
//   - 找不到叶子时的失败信息可读（列出当前可见叶子，便于判断是「被搜索过滤掉了」
//     还是「分组被折叠了」）。
//
// 这里**不做**任何分类推断：需要展开哪个分组，只能靠「逐个展开直到叶子出现」，
// 因为折叠态下 DOM 里根本没有条目可以判断归属（与 `catalogTree.ts` 的零前缀规则一致）。
import { fireEvent, screen, waitFor } from "@testing-library/react";
import { expect } from "vitest";

/** 叶子 testid 前缀（`CatalogCategoryTree.tsx` 的 `ItemRow`）。 */
const LEAF_PREFIX = "catalog-tree-leaf-";

/** 「分类未知」角标的 testid 前缀：同样以叶子前缀开头，取集合时必须排除。 */
const BADGE_PREFIX = "catalog-tree-leaf-badge-";

/** 变量叶子的 testid（变量 ID 直接拼接，不做转义/替换）。 */
export function catalogLeafTestId(variableId: string): string {
  return `${LEAF_PREFIX}${variableId}`;
}

function leafIdsOf(elements: HTMLElement[]): string[] {
  return elements
    .map((el) => el.getAttribute("data-testid") ?? "")
    .filter((id) => id.startsWith(LEAF_PREFIX) && !id.startsWith(BADGE_PREFIX))
    .map((id) => id.slice(LEAF_PREFIX.length));
}

/** 当前 DOM 中**可见**的叶子变量 ID（按分组顺序；不含角标、不含被折叠/被搜索过滤的条目）。 */
export function catalogLeafIds(): string[] {
  return leafIdsOf(
    Array.from(document.querySelectorAll<HTMLElement>(`button[data-testid^="${LEAF_PREFIX}"]`)),
  );
}

/** 当前被选中的叶子变量 ID（`data-selected="true"`；未选则为空数组）。 */
export function selectedCatalogLeafIds(): string[] {
  return leafIdsOf(
    Array.from(
      document.querySelectorAll<HTMLElement>(
        `button[data-testid^="${LEAF_PREFIX}"][data-selected="true"]`,
      ),
    ),
  );
}

/**
 * 尝试定位变量叶子：找不到就把**折叠中的**分组逐个展开再找。
 *
 * 注意：**被搜索词过滤掉的条目不会被这个兜底救回来**——过滤是产品行为，
 * 用例需要先清空搜索框（`nck-variable-search`）再选。
 */
function tryFindLeaf(variableId: string): HTMLElement | null {
  const testId = catalogLeafTestId(variableId);
  const direct = screen.queryByTestId(testId);
  if (direct) return direct;
  for (const toggle of screen.queryAllByTestId(/^catalog-tree-toggle-/)) {
    if (toggle.getAttribute("aria-expanded") === "false") {
      fireEvent.click(toggle);
      const leaf = screen.queryByTestId(testId);
      if (leaf) return leaf;
    }
  }
  return null;
}

function missingLeafError(variableId: string): Error {
  const visible = catalogLeafIds();
  return new Error(
    `分类树中找不到变量叶子「${variableId}」（当前可见叶子：${
      visible.length ? visible.join(", ") : "无"
    }）`,
  );
}

/**
 * 通过分类树选中变量（同步版：`fireEvent.click`）。
 *
 * 同步语义与旧的 `fireEvent.change(select, …)` 一致——点击后立即可以断言选中结果，
 * 不需要 `await`。目录已就绪的组件级用例用这个版本。
 */
export function selectVariableViaTree(variableId: string): HTMLElement {
  const leaf = tryFindLeaf(variableId);
  if (!leaf) throw missingLeafError(variableId);
  fireEvent.click(leaf);
  return leaf;
}

/**
 * 通过分类树选中变量（异步版：`userEvent.click`）。
 *
 * 与 `user.selectOptions` 等价：先等叶子出现（目录是异步加载的），再走真实的指针
 * 事件序列，适合与 `user.type` 等 userEvent 操作混排的用例。
 */
export async function selectVariableViaTreeWith(
  user: { click: (element: Element) => Promise<void> },
  variableId: string,
): Promise<HTMLElement> {
  const leaf = await waitFor(() => {
    const found = tryFindLeaf(variableId);
    expect(found).toBeTruthy();
    return found;
  });
  await user.click(leaf!);
  return leaf!;
}
