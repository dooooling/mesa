// Point Live Stream 唯一入口（STATE STREAM，不是 EVENT STREAM）。
// - transport：EventSource(`/api/v1/points/live`)，无 id/seq/replay；
//   首帧 mesa-points-snapshot（全量替换），后续 mesa-points-delta（整行合并）；
// - last-known：onerror/malformed 保留旧 points；malformed 同时翻 pointsError
//  （坏输入不伪装正常，下一合法帧自动恢复）；绝不 setPoints([])；
//   重连后 fresh snapshot 自动收敛；
// - STALE：无网络帧时由单次最近 deadline timer 翻转（无 1s 全表 tick）；
//   AgeCell 文本仍由 StaleClock 独立刷新（只改显示，不改视图）；
// - 与 useEventStream 无关：Point Live 不用 seq/Last-Event-ID/replay。
import { useEffect, useRef, useState } from "react";
import {
  nextStaleDeadlineMs,
  reconcilePointDelta,
  reconcilePointSnapshots,
  type PointSnapshotLike,
} from "../deviceModel";

export interface PointLiveRow extends PointSnapshotLike {
  // 后端 LatestEntry 全字段透传（value/type/quality/source_label/display_name…）。
  [k: string]: unknown;
}

export interface UsePointLiveStreamResult {
  points: PointLiveRow[];
  pointsError: boolean;
  /** staleRevision：STALE 翻转时 bump，调用方重算 derived（与 points 内容无关）。 */
  staleRevision: number;
  patchDisplayName: (endpoint_id: string, point_key: string, display_name: string | null) => void;
}

const SNAPSHOT_EVENT = "mesa-points-snapshot";
const DELTA_EVENT = "mesa-points-delta";

function decodePoints(data: string): PointLiveRow[] | null {
  try {
    const v = JSON.parse(data) as { points?: unknown };
    if (!v || !Array.isArray(v.points)) return null;
    // 最小形态校验：行 identity 必须完整，否则整帧丢弃（last-known）。
    for (const p of v.points as Array<Record<string, unknown>>) {
      if (typeof p !== "object" || p === null) return null;
      if (typeof (p as { endpoint_id?: unknown }).endpoint_id !== "string") return null;
      if (typeof (p as { point_id?: unknown }).point_id !== "number") return null;
    }
    return v.points as PointLiveRow[];
  } catch {
    return null;
  }
}

export function usePointLiveStream(): UsePointLiveStreamResult {
  const [points, setPoints] = useState<PointLiveRow[]>([]);
  const [pointsError, setPointsError] = useState(false);
  const [staleRevision, setStaleRevision] = useState(0);
  // subscribed 已删除：setState 在 effect 内标记就绪是多余重渲染，
  // 生产与测试都不需要（页面测试改用轮询 emit，不靠就绪门）。
  const gen = useRef(0);

  // 首帧 snapshot：全量替换（含删除收敛）。
  useEffect(() => {
    const id = ++gen.current;
    let cancelled = false;
    let src: EventSource | null = null;
    try {
      src = new EventSource("/api/v1/points/live");
    } catch {
      // jsdom 无 EventSource 时构造即抛：fail-closed（空快照 + 错误态）。
      setPointsError(true);
      return () => {
        cancelled = true;
      };
    }
    const onSnapshot = (e: MessageEvent) => {
      if (cancelled || gen.current !== id) return;
      const rows = decodePoints(e.data);
      // malformed：last-known 保留 + 错误态（坏输入不伪装正常）；
      // 下一合法帧自动恢复（setPointsError(false)）。
      if (!rows) {
        setPointsError(true);
        return;
      }
      const at = Date.now();
      setPoints((prev) => {
        const { points: merged, changed } = reconcilePointSnapshots(prev, rows, at, at);
        return changed ? merged : prev;
      });
      setPointsError(false);
    };
    // delta：整行合并，未提及行不动，不处理删除。
    const onDelta = (e: MessageEvent) => {
      if (cancelled || gen.current !== id) return;
      const rows = decodePoints(e.data);
      if (!rows) {
        setPointsError(true);
        return;
      }
      setPoints((prev) => {
        const { points: merged, changed } = reconcilePointDelta(prev, rows);
        return changed ? merged : prev;
      });
      setPointsError(false);
    };
    const onErr = () => {
      if (cancelled || gen.current !== id) return;
      // fail-closed：保留 last-known，只翻错误态；STALE 由 deadline timer 推进。
      setPointsError(true);
    };
    src.addEventListener(SNAPSHOT_EVENT, onSnapshot as EventListener);
    src.addEventListener(DELTA_EVENT, onDelta as EventListener);
    src.onerror = onErr;
    const live = src;
    return () => {
      cancelled = true;
      live.removeEventListener(SNAPSHOT_EVENT, onSnapshot as EventListener);
      live.removeEventListener(DELTA_EVENT, onDelta as EventListener);
      live.close();
    };
  }, []);

  // STALE deadline：只在真正翻转时 bump（无 1s tick、无网络请求）。
  useEffect(() => {
    let timer: number | undefined;
    const arm = () => {
      const now = Date.now();
      const wait = nextStaleDeadlineMs(points, now);
      if (wait === null) {
        // 已有越界但未翻转的行（reconcile 时 derived 旧）：立即 bump 一次收敛。
        // 无待翻转则静默。
        return undefined;
      }
      timer = window.setTimeout(() => {
        setStaleRevision((n) => n + 1);
      }, Math.max(0, wait));
      return timer;
    };
    const t = arm();
    return () => {
      if (t !== undefined) window.clearTimeout(t);
    };
  }, [points, staleRevision]);

  const patchDisplayName = (endpoint_id: string, point_key: string, display_name: string | null) => {
    setPoints((prev) =>
      prev.map((p) =>
        p.endpoint_id === endpoint_id && (p.key ?? p.point_key) === point_key
          ? { ...p, display_name: display_name ?? undefined }
          : p,
      ),
    );
  };

  return { points, pointsError, staleRevision, patchDisplayName };
}
