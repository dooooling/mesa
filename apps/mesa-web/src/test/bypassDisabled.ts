/**
 * 「禁用被绕过」回归探针（P0 专用，仅测试使用）。
 *
 * 背景：P0 要求「驱动清单失败 / 无可用驱动 / 描述符失败时，**所有提交路径**都必须
 * 失效」，并且要求证明「不能绕过禁用状态发出 POST/PUT」。也就是说，除了按钮
 * `disabled` 之外，提交函数自身还必须有前置守卫。
 *
 * 难点：React 与 AntD **双重**拦截被禁用按钮的 click ——
 * 1. React 的 `shouldPreventMouseEvent` 对 disabled 表单元素的 click 不再派发；
 * 2. AntD `Button` 的内部 `handleClick` 在 `mergedDisabled` 时直接 `preventDefault()` 早退。
 * 因此 `fireEvent.click` / 原生 `dispatchEvent` / `userEvent` 都无法触发被禁用的
 * 处理函数，交互测试**测不到**提交函数里的守卫（探针会静默通过，形成假绿）。
 *
 * 本探针沿 React 18 挂在 DOM 节点上的 `__reactFiber$*` 向上找到按钮**组件**
 * （AntD Button）fiber，直接调用其 `onClick` prop，从而精确模拟「disabled 不再
 * 生效 / 有人程序化调用提交」的场景：若提交函数没有自己的守卫，该调用会真的发出
 * 写请求，测试随即失败。
 *
 * 返回值表示**是否真的调用到了一个组件级 onClick**：调用方必须断言其为 true，
 * 否则探针没找到目标时测试会假绿。
 */
interface FiberLike {
  type: unknown;
  memoizedProps: Record<string, unknown> | null;
  return: FiberLike | null;
}

/** 触发按钮组件级 onClick（绕过 disabled）。返回是否成功调用。 */
export function invokeDisabledButton(button: Element): boolean {
  const key = Object.keys(button).find((k) => k.startsWith("__reactFiber$"));
  if (!key) return false;
  const hostFiber = (button as unknown as Record<string, FiberLike | undefined>)[key] ?? null;
  // 宿主 <button> 自身的 onClick 是 AntD 的内部处理函数，会在 mergedDisabled 处
  // 早退，无法用来验证守卫；必须从它的父级开始找**组件**fiber。
  let fiber = hostFiber?.return ?? null;
  while (fiber) {
    const props = fiber.memoizedProps;
    if (typeof fiber.type !== "string" && props && typeof props.onClick === "function") {
      (props.onClick as (event: unknown) => void)({
        preventDefault() {},
        stopPropagation() {},
        nativeEvent: {},
      });
      return true;
    }
    fiber = fiber.return;
  }
  return false;
}

/** 按无障碍名取按钮并绕过 disabled 触发其组件级 onClick。 */
export function invokeDisabledButtonByName(name: RegExp): boolean {
  const buttons = Array.from(document.querySelectorAll("button"));
  const target = buttons.find((b) => name.test((b.textContent ?? "").replace(/\s+/g, "")));
  if (!target) return false;
  return invokeDisabledButton(target);
}
