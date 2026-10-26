// 采集草稿的**应用内离开守卫**（§7-7 定案：离开页面要提示，不静默丢弃）。
//
// 为什么要有这一层（第 4 步修复）：第 2 步只把守卫接在「设备 Tab」上，用户实测
// 「改周期 → 点侧栏『总览』→ 无确认 → 回到采集页草稿变回 1000」。漏掉的离开路径有
// 三条：侧栏导航、面包屑、浏览器前进/后退。它们分属应用外壳（`App.tsx`）、设备页头
// （`DeviceHeader.tsx`）与浏览器本身，因此守卫的**状态**必须提在外壳层共享，判定
// 与文案仍复用 `acquisitionDraftGuard.ts` 的 `confirmDiscardDraft`（不新写一套）。
//
// 覆盖面与机制：
// 1. 侧栏：`App.tsx` 的 Menu onClick 走 `useGuardedNavigate()`；
// 2. 面包屑：`GuardedLink`（设备页头「设备」返回）；
// 3. 设备 Tab：`DeviceTabNav` 走同一个 `useGuardedNavigate()`；
// 4. 浏览器前进/后退：`DraftGuardProvider` 内有未保存草稿时压一个**同 URL 哨兵条目**，
//    后退先落回同一 URL（路由因此不变、草稿不丢），此时弹确认；取消则补回哨兵留在本页，
//    确认才真正 `history.go(-1)` 离开。
//
// 为什么不用 router 的 `useBlocker`：本项目的路由是 `<BrowserRouter>` + `<Routes>`
// 声明式路由（`main.tsx:66`、`App.tsx:61-79`），`useBlocker` 只在 data router
// （`createBrowserRouter`）下可用；改成 data router 会牵动全部以 `MemoryRouter`
// 渲染 `<App/>` 的既有测试。本模块因此用「哨兵条目 + popstate」实现同一效果。
//
// 已知边界（不假装已覆盖）：哨兵条目依赖 `history.pushState`；若运行环境禁止它，
// 浏览器前进/后退退化为无法拦截（应用内三条路径仍受保护）。
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { Link, useNavigate, type LinkProps } from "react-router-dom";
import { confirmDiscardDraft } from "../acquisitionDraftGuard";

interface DraftGuardValue {
  /** 当前是否有未保存的采集草稿。 */
  dirty: boolean;
  /** 有草稿时问一次；返回 true 表示用户确认放弃并继续。 */
  confirmLeave: (action: string) => Promise<boolean>;
  /** 采集页上报草稿状态（唯一来源，避免两个真值）。 */
  reportDirty: (dirty: boolean) => void;
}

const DraftGuardContext = createContext<DraftGuardValue | null>(null);

/** 离开动作的统一文案（Tab / 侧栏 / 面包屑 / 前进后退都用它）。 */
export const DRAFT_LEAVE_ACTION = "离开采集页";

/**
 * 外壳层守卫状态容器：`App.tsx` 里包住侧栏 + 内容区。
 *
 * 放在外壳而不是设备页：侧栏与面包屑都不在设备页里，只有提到共同父层才能
 * 「同一守卫覆盖全部应用内离开路径」。
 */
export function DraftGuardProvider({ children }: { children: ReactNode }) {
  const [dirty, setDirty] = useState(false);
  const reportDirty = useCallback((next: boolean) => setDirty(next), []);
  const confirmLeave = useCallback((action: string) => confirmDiscardDraft(dirty, action), [dirty]);
  useBrowserBackGuard(dirty, confirmLeave);
  const value = useMemo<DraftGuardValue>(
    () => ({ dirty, confirmLeave, reportDirty }),
    [dirty, confirmLeave, reportDirty],
  );
  return <DraftGuardContext.Provider value={value}>{children}</DraftGuardContext.Provider>;
}

/** 读取守卫上下文（测试/独立渲染时允许缺失，视为「无草稿」）。 */
export function useDraftGuard(): DraftGuardValue | null {
  return useContext(DraftGuardContext);
}

/**
 * 采集页上报自己的草稿状态。
 *
 * 没有 Provider 时静默无效（第 2 步起就有直接渲染 `DeviceAcquisition` 的测试，
 * 不该因为缺 Provider 而抛错）。
 */
export function useDraftGuardReporter(dirty: boolean): void {
  const ctx = useDraftGuard();
  const report = ctx?.reportDirty;
  useEffect(() => {
    report?.(dirty);
    // 卸载时复位：草稿随组件一起消失，守卫不能继续拦着用户。
    return () => report?.(false);
  }, [report, dirty]);
}

/**
 * 守卫版导航：有未保存草稿时先确认，取消则该次导航**不发生**。
 *
 * 没有 Provider（或无草稿）时与普通 `navigate` 等价（不多一次微任务，行为不变）。
 */
export function useGuardedNavigate(): (to: string, action?: string) => void {
  const ctx = useDraftGuard();
  const nav = useNavigate();
  return useCallback(
    (to: string, action: string = DRAFT_LEAVE_ACTION) => {
      if (!ctx) {
        nav(to);
        return;
      }
      if (!ctx.dirty) {
        nav(to);
        return;
      }
      void (async () => {
        if (await ctx.confirmLeave(action)) nav(to);
      })();
    },
    [ctx, nav],
  );
}

/** 守卫版链接（面包屑等）：点击先过守卫，取消则停在本页。 */
export function GuardedLink({
  to,
  action = DRAFT_LEAVE_ACTION,
  children,
  onClick,
  ...rest
}: LinkProps & { action?: string }) {
  const go = useGuardedNavigate();
  return (
    <Link
      {...rest}
      to={to}
      onClick={(event) => {
        onClick?.(event);
        if (event.defaultPrevented) return;
        // 统一由守卫决定跳不跳：Link 自身不再直接导航。
        event.preventDefault();
        go(String(to), action);
      }}
    >
      {children}
    </Link>
  );
}

/** 哨兵条目的标记键（只用于调试可读性，不参与判定）。 */
const SENTINEL = "__mesaDraftGuard";

/**
 * 浏览器前进/后退守卫（同 URL 哨兵条目 + popstate）。
 *
 * 语义：有草稿时后退**先落在同一 URL**，此时弹确认——
 * - 取消：路由没变、组件没卸载，草稿原样保留；补回哨兵，下次后退仍会问；
 * - 确认：`history.go(-1)` 真正离开（哨兵只多一条同 URL 记录，不改变落点）。
 */
function useBrowserBackGuard(dirty: boolean, confirmLeave: (action: string) => Promise<boolean>): void {
  // 用 ref 读最新判定，避免每次 dirty 变化都重建监听（条目只压一次）。
  const confirmRef = useRef(confirmLeave);
  confirmRef.current = confirmLeave;
  useEffect(() => {
    if (!dirty || typeof window === "undefined" || !window.history?.pushState) return;
    const href = window.location.href;
    const pushSentinel = () => {
      try {
        window.history.pushState({ ...(window.history.state ?? {}), [SENTINEL]: true }, "", href);
      } catch {
        // 环境禁止 pushState：只能退化为「应用内三条路径受保护」。
      }
    };
    pushSentinel();
    const onPop = () => {
      // URL 真的变了 ⇒ 没有经过哨兵（或环境不支持）：不再追问，避免把用户拖回来。
      if (window.location.href !== href) return;
      void (async () => {
        const leave = await confirmRef.current(DRAFT_LEAVE_ACTION);
        if (leave) {
          window.history.go(-1);
        } else {
          pushSentinel();
        }
      })();
    };
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, [dirty]);
}
