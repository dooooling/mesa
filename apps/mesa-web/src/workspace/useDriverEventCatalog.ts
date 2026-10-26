// 「事件订阅」区域是否该出现 —— 判据只有 Descriptor 的**事件能力目录**。
//
// 为什么单独一层（§2.5-8 / §7-8 定案）：事件订阅是否可用是驱动**声明**出来的能力，
// 与驱动是谁无关。这里因此只读 `descriptor.events.streams`（`types.ts:91`，老驱动
// 缺省时按空目录处理，与 Rust serde(default) 对齐），**不出现任何 `driver_id` 判断**。
//
// 与 `EventTaskEditor.tsx:129`、`:330-331` 的判据同源（`descriptor?.events?.streams ?? []`）：
// 编辑器内部仍按同一份目录决定「未声明事件流」的兜底展示，本 hook 只是把
// 「要不要给出这块区域」提前到页面层，避免不支持事件的事件卡片常驻主页。
import { useEffect, useState } from "react";
import { api } from "../api";
import type { DriverDescriptor, EventStreamDescriptor } from "../types";

export interface EventCatalogView {
  /** loading：还没结论（不渲染，避免闪一下再消失）；error：读不到描述。 */
  state: "loading" | "ready" | "error";
  /** 驱动声明的事件流（`descriptor.events.streams`；缺省为空数组）。 */
  streams: EventStreamDescriptor[];
  /** 读取失败的原始信息（技术详情/重试用）。 */
  error: string;
  /** 手动重试（描述读取失败时给出入口，不静默隐藏真实问题）。 */
  reload: () => void;
}

/**
 * 读取某个驱动的事件能力目录。
 *
 * `driverId` 为空（没有可用连接）时保持 loading：此时谈不上「不支持事件」，
 * 页面层按不渲染处理即可。
 */
export function useDriverEventCatalog(driverId: string | null): EventCatalogView {
  const [state, setState] = useState<EventCatalogView["state"]>("loading");
  const [streams, setStreams] = useState<EventStreamDescriptor[]>([]);
  const [error, setError] = useState("");
  const [nonce, setNonce] = useState(0);

  useEffect(() => {
    if (!driverId) {
      setState("loading");
      setStreams([]);
      setError("");
      return;
    }
    // 切换连接/重试都会换 controller：旧响应不得落到新 Endpoint 的区域上。
    const controller = new AbortController();
    let cancelled = false;
    setState("loading");
    setError("");
    api
      .getDescriptor(driverId, controller.signal)
      .then((raw: unknown) => {
        if (cancelled) return;
        const streams = ((raw as DriverDescriptor)?.events?.streams ?? []) as EventStreamDescriptor[];
        setStreams(streams);
        setState("ready");
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setStreams([]);
        setError(e instanceof Error ? e.message : String(e));
        setState("error");
      });
    return () => {
      cancelled = true;
      controller.abort();
    };
  }, [driverId, nonce]);

  return { state, streams, error, reload: () => setNonce((v) => v + 1) };
}
