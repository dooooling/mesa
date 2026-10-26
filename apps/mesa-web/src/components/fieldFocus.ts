// 字段级定位（服务端 `issues[].path` 指到 `...parameters.<field>` 时的落点动作）。
//
// 为什么要一个共用实现：套用「展开折叠区 → 高亮 → 滚动到锚点」这三步的地方有三处
// ——NCK 原生表单（`NckNativeFields.tsx`）、通用资源选择表单（`ResourcePickerAntd.tsx`）
// 的字段，以及未来任何 Descriptor 驱动的表单。三处各写一遍会出现"有的会滚动、
// 有的只高亮不滚动"的差异；这里只保留**一套**动作顺序。
//
// 为什么必须"有限次重试"：抽屉/折叠区里的字段比触发定位的那次提交**晚一次渲染**
// （antd Drawer 在 open 后才挂载子树、高级字段展开后才渲染），当场
// `getElementById` 必然是 null。这里只重试有限次，拿不到锚点就放弃——
// **不猜元素**、也不抛错。
import { useEffect } from "react";

/** 重试次数与间隔（约 320ms 上限：足够跨过一两次渲染，又不会长期占用定时器）。 */
const LOCATE_ATTEMPTS = 20;
const LOCATE_INTERVAL_MS = 16;

/**
 * 把某个字段锚点（`document.getElementById(anchorId)`）滚动到视口中央。
 *
 * 返回取消函数（组件卸载/定位目标变化时调用），避免定时器泄漏。
 * 锚点始终不存在（字段不在 Schema、折叠区没展开、抽屉没开）时静默放弃。
 */
export function locateFieldAnchor(anchorId: string): () => void {
  let cancelled = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let attempts = 0;
  const run = () => {
    if (cancelled) return;
    const anchor = document.getElementById(anchorId);
    if (anchor) {
      anchor.scrollIntoView?.({ block: "center" });
      return;
    }
    attempts += 1;
    if (attempts <= LOCATE_ATTEMPTS) timer = setTimeout(run, LOCATE_INTERVAL_MS);
  };
  run();
  return () => {
    cancelled = true;
    if (timer) clearTimeout(timer);
  };
}

/**
 * 受控的高亮 + 定位 effect（表单组件直接用）。
 *
 * `anchorId` 为 null ⇒ 不做任何事（没有定位目标，也不残留高亮）。
 */
export function useFieldAnchorFocus(anchorId: string | null): void {
  useEffect(() => {
    if (!anchorId) return;
    return locateFieldAnchor(anchorId);
  }, [anchorId]);
}
