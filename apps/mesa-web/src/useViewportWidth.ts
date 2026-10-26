// 视口宽度：窄屏自适应的**唯一**输入（纯展示层，无任何驱动判断）。
//
// 为什么需要单独一层：
// - 抽屉默认宽 640px（§4.1-1 / §7-5），点位表三列最小宽度约 586px；在 480px
//   级窄屏上两者都会溢出到页面外，用户必须横向拖动才点得到「编辑 / 更多」，
//   这正是第 3 步验收里的窄屏失败信号。
// - 但「窄屏」不属于任何具体业务组件：它是**布局输入**。所以这里只提供宽度数值
//   与两个纯函数（抽屉收敛、宽窄判定），由通用组件各自决定怎么用——组件里依然
//   不出现驱动名、协议字段或资源 ID 判断（§2.8）。
//
// 本期不做「抽屉宽度拖拽」：宽度只随视口收敛，不接受用户调整（§7-5 定案）。
import { useEffect, useState } from "react";

/** 视口宽度 ≤ 该阈值即视为窄屏（与抽屉默认宽 640px 同一量级的保守取值）。 */
export const NARROW_VIEWPORT_MAX = 768;

/** 是否窄屏。宽度未知（0 / SSR）一律按宽屏处理：宁可保持默认布局，也不猜。 */
export function isNarrowViewport(width: number): boolean {
  return width > 0 && width <= NARROW_VIEWPORT_MAX;
}

/**
 * 抽屉实际宽度：桌面保持 `desktopWidth`，窄屏收敛到视口宽度（不横向溢出）。
 *
 * 为什么用 `min` 而不是百分比：桌面上必须**恰好**是 640px（定案值，便于目视对齐），
 * 只有视口比它更窄时才让位给视口。宽度未知时保持默认值，避免首帧闪成 0。
 */
export function fitDrawerWidth(desktopWidth: number, viewportWidth: number): number {
  if (!(viewportWidth > 0)) return desktopWidth;
  return Math.min(desktopWidth, viewportWidth);
}

/**
 * 订阅视口宽度（`window.innerWidth`）。
 *
 * 只监听 `resize`：旋转/拖动窗口都会触发它；不引入 matchMedia 断点表，避免出现
 * 「CSS 断点」与「JS 阈值」两套口径不一致。首帧同步读一次，避免先渲染宽屏再跳变。
 */
export function useViewportWidth(): number {
  const [width, setWidth] = useState<number>(() => readViewportWidth());
  useEffect(() => {
    if (typeof window === "undefined") return;
    const onResize = () => setWidth(readViewportWidth());
    // 挂载时再读一次：挂载前的窗口变化（如 devtools 停靠）不会触发 resize。
    onResize();
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  return width;
}

function readViewportWidth(): number {
  if (typeof window === "undefined") return 0;
  const width = window.innerWidth;
  return Number.isFinite(width) && width > 0 ? width : 0;
}
