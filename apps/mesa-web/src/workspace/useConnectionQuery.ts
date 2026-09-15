// 页面局部 ?connection= 管理：读/验/写本页的连接选择，不做跨 Tab 同步。
// - Data/Events（mode "all"）：缺失/非法 → 全部（null）；合法 → 单连接。
// - Acquisition/Diagnostics（mode "single"）：合法 → 单连接；缺失/非法 →
//   第一个连接；无连接 → null（页面显式 Empty State）。
// Tab 切换不保留 query（DeviceTabNav 用 Link 直达，不带参）。
import { useCallback, useMemo } from "react";
import { useSearchParams } from "react-router-dom";

export function useConnectionQuery(args: {
  endpointIds: string[];
  mode: "all" | "single";
  /** 清单未就绪前不写 URL（避免空清单误删合法参数）。 */
  ready: boolean;
}): { selected: string | null; select: (id: string | null) => void } {
  const { endpointIds, mode, ready } = args;
  const [params, setParams] = useSearchParams();
  const raw = (params.get("connection") ?? "").trim();

  const selected = useMemo(() => {
    if (raw && endpointIds.includes(raw)) return raw;
    if (mode === "all") return null;
    return endpointIds.length ? endpointIds[0] : null;
  }, [raw, endpointIds, mode]);

  const select = useCallback(
    (id: string | null) => {
      if (!ready) return;
      const next = new URLSearchParams(params);
      if (id === null) next.delete("connection");
      else next.set("connection", id);
      setParams(next, { replace: true });
    },
    [ready, params, setParams],
  );

  return { selected, select };
}
