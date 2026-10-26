// 采集草稿的**应用内离开守卫**（§7-7 定案：离开页面要提示，不静默丢弃）。
//
// 为什么要有这一层（第 4 步修复）：第 2 步只把守卫接在「设备 Tab」上，用户实测
// 「改周期 → 点侧栏『总览』→ 无确认 → 回到采集页草稿变回 1000」。漏掉的离开路径有
// 三条：侧栏导航、面包屑、浏览器前进/后退。它们分属应用外壳（`App.tsx`）、设备页头
// （`DeviceHeader.tsx`）与浏览器本身，因此守卫的**状态**必须提在外壳层共享，判定
// 与文案仍复用 `acquisitionDraftGuard.ts` 的 `confirmDiscardDraft`（不新写一套）。
//
// 覆盖面与机制：
// 1. 侧栏：`App.tsx:65` 的 Menu onClick 走 `useGuardedNavigate()`；
// 2. 面包屑：`GuardedLink`（设备页头「设备」返回）；
// 3. 设备 Tab：`DeviceTabNav` 走同一个 `useGuardedNavigate()`；
// 4. 浏览器前进/后退：`useBrowserBackGuard`（本文件下半部）。
//
// 为什么不用 router 的 `useBlocker`：本项目的路由是 `<BrowserRouter>` + `<Routes>`
// 声明式路由（`main.tsx:66`、`App.tsx:74-92`），`useBlocker` 只在 data router
// （`createBrowserRouter`）下可用；改成 data router 会牵动全部以 `MemoryRouter`
// 渲染 `<App/>` 的既有测试。本模块因此用「锚点条目 + 同 URL 哨兵 + popstate」实现
// 同一效果（细节见 `useBrowserBackGuard`）。
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
import { Link, useLocation, useNavigate, type LinkProps } from "react-router-dom";
import { confirmDiscardDraft } from "../acquisitionDraftGuard";
import { DraftStashProvider, useDraftStash } from "../draftStash";

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
 * 「同一守卫覆盖全部应用内离开路径」。同时把**草稿副本容器**（`DraftStashProvider`）
 * 套在同一层：它包住整个路由内容，因此路由怎么变都不会销毁副本（纵深防御）。
 */
export function DraftGuardProvider({ children }: { children: ReactNode }) {
  return (
    <DraftStashProvider>
      <DraftGuardState>{children}</DraftGuardState>
    </DraftStashProvider>
  );
}

function DraftGuardState({ children }: { children: ReactNode }) {
  const [dirty, setDirty] = useState(false);
  const reportDirty = useCallback((next: boolean) => setDirty(next), []);
  const confirmLeave = useCallback((action: string) => confirmDiscardDraft(dirty, action), [dirty]);
  const stash = useDraftStash();
  // 用户确认放弃后清掉草稿副本：否则下次进页面会把"已放弃的草稿"复活。
  const clearStash = useCallback(() => stash?.clear(), [stash]);
  useBrowserBackGuard(dirty, confirmLeave, clearStash);
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
 *
 * 卸载时复位为 false：草稿随组件一起消失时守卫不能继续拦着用户。注意——「草稿
 * 内容」不会真的消失，副本在 `draftStash`（路由之外），重新挂载会恢复。
 */
export function useDraftGuardReporter(dirty: boolean): void {
  const ctx = useDraftGuard();
  const report = ctx?.reportDirty;
  useEffect(() => {
    report?.(dirty);
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
 * 「确认后那次后退没能发生」的兜底等待时间。
 *
 * 正常浏览器里 `history.go()` 的 popstate 在几十毫秒内到达；超过这个时间还没有
 * popstate，就认为导航没有发生（无处可退），必须把守卫恢复成「仍武装」。
 */
const LEAVE_FALLBACK_MS = 400;

/** 路由落点（只留把内容拉回采集页所需的三段）。 */
interface AnchorRoute {
  pathname: string;
  search: string;
  hash: string;
}

/**
 * 浏览器后退守卫的运行时状态（放在 `useRef` 里，**跨 effect 重跑保留**）。
 *
 * 为什么要跨重跑保留：采集页短暂卸载（跨步遍历的中间态）会让上报的 `dirty`
 * 抖成 false 再回 true；若待确认状态跟着 effect 一起重建，已经弹出的确认框就会
 * 「确认了却没走」或「取消后又弹一次」。
 */
interface BackGuardState {
  /** 采集页条目（锚点）的完整 URL；空串 = 守卫未武装。 */
  anchorHref: string;
  /** 锚点对应的**路由**落点：取消/中和后据此把内容与地址一起拉回来。 */
  anchorRoute: AnchorRoute | null;
  /** 是否已压入哨兵条目且尚未被消耗。 */
  sentinel: boolean;
  /** 是否已经弹过确认（待确认期间不重复弹第二个框）。 */
  asking: boolean;
  /** 已确认放弃：下一次 popstate 放行（不得再拦）。 */
  leaving: boolean;
  /**
   * 跨步遍历时记下的「用户原本想去的位置」，确认后一次性跳过去。
   *
   * 存**路径**（`pathname + search + hash`）而不是 `location.href`：后者带
   * `http://host` 前缀，交给路由的 `navigate()` 会被当成相对路径解析，落到
   * `/http:/host/...` 这种错误地址上。
   */
  targetPath: string | null;
}

/**
 * 浏览器前进/后退守卫：**锚点条目 + 同 URL 哨兵 + popstate**。
 *
 * 为什么这样设计（根因：本项目是声明式路由，popstate 时路由已经变了）：
 * `BrowserRouter` 在 `popstate` 里同步更新 `location`（`useLayoutEffect` 订阅，
 * `@remix-run/router` 的 `handlePop`），而草稿本来是采集页组件的 `useState`。
 * 也就是说：**只要 URL 真的变了，采集页就已经被卸载、草稿已经没了**，事后没有
 * 任何"取消"能把它变回来（旧实现 `:169` 遇到 URL 变化就直接放行，正是漏掉了这点）。
 *
 * 因此策略是「让 URL 根本不变」：
 * 1. 有草稿时在当前条目之后压一条**同 URL 哨兵**。后退先落回同一 URL（路由不变、
 *    组件不卸载、草稿不丢），此时弹确认；
 * 2. 每处理完一次 popstate 就**补回哨兵**。于是连续后退（含待确认期间再后退）
 *    每一次都只落到同一 URL，逐个被中和——不会穿透；
 * 3. 取消：URL 与路由内容本来就没离开，无需任何"回滚"；若曾发生跨步遍历，
 *    在 popstate 当场就把地址与内容一起拉回采集页（`repair`），因此不会出现
 *    「地址是采集页、内容是概览页」；
 * 4. 确认：当前条目是哨兵、锚点在它前一条 ⇒ `history.go(-2)` 一次落在用户原本
 *    想去的位置（只走一步，不多跳不漏跳）；跨步遍历时改用记下的 `targetPath`
 *    导航一次；
 * 5. 纵深防御：草稿内容另有副本（`draftStash.tsx`，挂在路由之外），即使某次
 *    中间态真的把采集页卸载了，重新挂载也会恢复成未保存草稿。
 */
function useBrowserBackGuard(
  dirty: boolean,
  confirmLeave: (action: string) => Promise<boolean>,
  onConfirmedLeave: () => void,
): void {
  const location = useLocation();
  const navigate = useNavigate();
  // 全部经 ref 读取最新值：监听只在挂载时安装一次，不受渲染次数影响。
  const confirmRef = useRef(confirmLeave);
  confirmRef.current = confirmLeave;
  const onLeaveRef = useRef(onConfirmedLeave);
  onLeaveRef.current = onConfirmedLeave;
  const locationRef = useRef(location);
  locationRef.current = location;
  const navigateRef = useRef(navigate);
  navigateRef.current = navigate;
  const dirtyRef = useRef(dirty);
  dirtyRef.current = dirty;
  const state = useRef<BackGuardState>({
    anchorHref: "",
    anchorRoute: null,
    sentinel: false,
    asking: false,
    leaving: false,
    targetPath: null,
  });

  // 压哨兵：保持路由自己写入的 `history.state`（里面有它算 delta 用的 `idx`），
  // 只额外加一个标记键，避免把路由状态整体覆盖掉。
  const pushSentinel = useCallback(() => {
    const s = state.current;
    if (!s.anchorHref) return;
    try {
      window.history.pushState({ ...(window.history.state ?? {}), [SENTINEL]: true }, "", s.anchorHref);
      s.sentinel = true;
    } catch {
      // 环境禁止 pushState：只能退化为「应用内三条路径受保护」（见文件头「已知边界」）。
    }
  }, []);

  // 把「地址」与「路由内容」**一起**拉回采集页。用路由自己的 navigate(replace) 而不是
  // 裸 `pushState`：后者只改地址栏，路由内容仍停在别处——那正是用户实测到的
  // 「地址栏显示采集页、页面内容却是概览页」。
  const repair = useCallback(() => {
    const s = state.current;
    if (!s.anchorRoute) return;
    navigateRef.current(s.anchorRoute, { replace: true });
    s.sentinel = false;
    pushSentinel();
  }, [pushSentinel]);

  const ask = useCallback(() => {
    const s = state.current;
    if (s.asking) return;
    s.asking = true;
    void (async () => {
      const leave = await confirmRef.current(DRAFT_LEAVE_ACTION);
      s.asking = false;
      if (!leave) {
        // 取消：常路径下什么都没变（URL / 内容 / 草稿都在原位）；只有跨步遍历需要
        // 再复核一次（幂等，正常路径下不会触发）。
        if (s.anchorHref && window.location.href !== s.anchorHref) repair();
        return;
      }
      s.leaving = true;
      // 用户明确放弃 ⇒ 草稿副本一并清掉（否则重进页面会复活已放弃的草稿）。
      onLeaveRef.current();
      const target = s.targetPath;
      s.targetPath = null;
      if (target) {
        // 跨步遍历过：锚点之前的那条记录已在 `repair` 里被改写，按记下的去向
        // 导航**一次**（不多跳、不漏跳）。这里不会产生 popstate，故立即复位标志。
        navigateRef.current(target);
        s.leaving = false;
        s.sentinel = false;
        return;
      }
      // 常路径：当前条目 = 哨兵，锚点在它前一条 ⇒ 后退两步正好落在「采集页之前」
      // 的条目，只发生一次导航（不多跳、不漏跳）。
      // 哨兵没压上（环境禁止 pushState）时，当前条目就是采集页本身 ⇒ 只退一步。
      window.history.go(s.sentinel ? -2 : -1);
      // 兜底：若这一步后退没能发生（例如采集页就是历史里的第一条条目、无处可退），
      // 上面那次导航不会产生 popstate。此时必须自己回到"仍武装"的状态，
      // 否则 `leaving` 会一直为真，把之后的后退全部放行（守卫形同关闭）。
      // 注意**不要**再压一条哨兵：当前条目仍是原来那一条哨兵，叠出同 URL 的第二层
      // 会让之后的 `go(-2)` 少退一页。
      window.setTimeout(() => {
        if (!s.leaving) return;
        s.leaving = false;
      }, LEAVE_FALLBACK_MS);
    })();
  }, [repair, pushSentinel]);

  // 监听只挂一次：待确认期间若因草稿短暂消失/恢复而重建监听，已弹出的确认框就会
  // 失去它的守卫。
  useEffect(() => {
    if (typeof window === "undefined" || !window.history?.pushState) return;
    const onPop = () => {
      const s = state.current;
      if (s.leaving) {
        // 我们主动发起的离开：放行本次导航，并清掉守卫状态。
        s.leaving = false;
        s.sentinel = false;
        s.targetPath = null;
        return;
      }
      if (!dirtyRef.current || !s.anchorHref) return;
      const href = window.location.href;
      if (href === s.anchorHref) {
        // 落在「采集页条目」或「哨兵条目」：URL 没离开采集页 ⇒ 路由不变、组件不卸载。
        // 先补回哨兵，保证下一次后退（包括连续后退）仍落在同一 URL。
        s.sentinel = false;
        pushSentinel();
      } else {
        // 跨步遍历（浏览器把连点合并成一次多步后退）：记下用户想去的位置，
        // 并**立即**中和——地址与内容一起拉回采集页。**每一次** popstate 都这样做。
        s.targetPath = `${window.location.pathname}${window.location.search}${window.location.hash}`;
        repair();
      }
      ask();
    };
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, [ask, pushSentinel, repair]);

  // 草稿出现 ⇒ 武装（记锚点 + 压哨兵）；草稿消失 ⇒ 收尾。
  useEffect(() => {
    if (typeof window === "undefined" || !window.history?.pushState) return;
    const s = state.current;
    if (dirty) {
      if (window.location.href !== s.anchorHref) {
        // 新的采集页条目（或上一次离开后重新进入）：重设锚点。
        s.anchorHref = window.location.href;
        s.anchorRoute = {
          pathname: locationRef.current.pathname,
          search: locationRef.current.search,
          hash: locationRef.current.hash,
        };
        s.sentinel = false;
        s.targetPath = null;
        s.leaving = false;
      }
      // 同一页面上「保存后又有修改」等情形锚点不变，且哨兵可能仍在 ⇒ 不重复压。
      if (!s.sentinel) pushSentinel();
      return;
    }
    // 草稿已无修改/已保存/已放弃：只要已经不在采集页就把守卫复位（下次重新武装）。
    if (s.anchorHref && window.location.href !== s.anchorHref) {
      s.anchorHref = "";
      s.anchorRoute = null;
      s.sentinel = false;
      s.targetPath = null;
      s.leaving = false;
      s.asking = false;
    }
  }, [dirty, pushSentinel]);
}
