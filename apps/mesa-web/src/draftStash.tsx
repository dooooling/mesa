// 采集草稿的**路由之外**暂存（离开守卫的纵深防御，§7-7 第 5 步）。
//
// 为什么必须有这一层：本项目是 `<BrowserRouter>` + `<Routes>` 声明式路由
// （`main.tsx:66`、`App.tsx:74-92`），浏览器原生后退触发 `popstate` 时
// **路由已经变了**——React 会立刻卸载采集页组件，组件内 `useState` 的草稿随之
// 消失，事后无法再"取消"。把草稿内容的副本放进一个**不受路由影响**的容器
// （Provider 挂在外壳层，路由内容只是它的 children），短暂的路由变化就不可能
// 销毁草稿：重新挂载时按同一 key 取回并恢复成「未保存草稿」。
//
// 边界（刻意不做的事）：
// - 不存多份/历史副本：只有「最近一次草稿」，`take` 是**一次性消费**（取走即删除），
//   避免下次进入页面时复活一份早已放弃的旧草稿；
// - 不参与"要不要离开"的判定：判定仍在 `draftNavGuard.tsx` / `acquisitionDraftGuard.ts`，
//   这里只解决"草稿内容会不会丢"。
import { createContext, useContext, useMemo, useRef, type ReactNode } from "react";

/** 草稿副本的读写接口（一次性消费，避免旧草稿复活）。 */
export interface DraftStashApi {
  /** 写入/覆盖某个 key 的草稿副本。 */
  put: (key: string, payload: unknown) => void;
  /** 取走副本（取走后即删除）；没有副本返回 null。 */
  take: (key: string) => unknown | null;
  /** 丢弃某个 key 的副本（草稿已无修改/已放弃）。 */
  drop: (key: string) => void;
  /** 全部丢弃（用户确认放弃离开时用）。 */
  clear: () => void;
}

const DraftStashContext = createContext<DraftStashApi | null>(null);

/**
 * 草稿副本容器。
 *
 * 状态放在 `useRef` 的 Map 里：写入只发生在草稿变化的事件回调/effect 中，
 * 不需要触发重渲染（副本不是渲染数据源，恢复时才被读一次）。
 */
export function DraftStashProvider({ children }: { children: ReactNode }) {
  const store = useRef(new Map<string, unknown>());
  const api = useMemo<DraftStashApi>(
    () => ({
      put: (key, payload) => {
        store.current.set(key, payload);
      },
      take: (key) => {
        if (!store.current.has(key)) return null;
        const value = store.current.get(key);
        store.current.delete(key);
        return value ?? null;
      },
      drop: (key) => {
        store.current.delete(key);
      },
      clear: () => {
        store.current.clear();
      },
    }),
    [],
  );
  return <DraftStashContext.Provider value={api}>{children}</DraftStashContext.Provider>;
}

/**
 * 读取草稿副本接口。
 *
 * 没有 Provider（大量测试直接渲染 `EndpointAcquisitionPane`）时返回 null：
 * 调用方必须把它当成「不做副本」而不是错误，既有行为零变化。
 */
export function useDraftStash(): DraftStashApi | null {
  return useContext(DraftStashContext);
}

/**
 * 稳定的副本 key：一个 Endpoint 一份草稿。
 *
 * 刻意只按 `endpointId`（Endpoint id 全局唯一）：副本是"留给同一个编辑对象"的内容，
 * 与驱动类型无关——这里**不**按 `driver_id` 分类，也不做任何驱动分支。
 */
export function draftStashKey(endpointId: string): string {
  return `endpoint:${endpointId}`;
}

/** 供组件使用的空实现（无 Provider 时避免到处写 `?.`）。 */
export const NO_DRAFT_STASH: DraftStashApi = {
  put: () => {},
  take: () => null,
  drop: () => {},
  clear: () => {},
};

/** 便捷封装：无 Provider 时退化为空实现（接口引用稳定，可直接进依赖数组）。 */
export function useDraftStashOrDefault(): DraftStashApi {
  const api = useDraftStash();
  return api ?? NO_DRAFT_STASH;
}
