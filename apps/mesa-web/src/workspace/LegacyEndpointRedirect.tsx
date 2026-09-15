// M1 兼容重定向：老 Endpoint 深层 URL → Device Workspace。
// 旧入口 `/devices/:deviceId/endpoints/:endpointId` 不再是新导航的一部分，
// 但老书签/外链仍可达：重定向到所属设备的概览 tab，并保留 connection 上下文
//（`?connection=`），用户不丢失位置。
import { useEffect } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";

export function LegacyEndpointRedirect() {
  const { deviceId = "", endpointId = "" } = useParams();
  const [params] = useSearchParams();
  const nav = useNavigate();

  useEffect(() => {
    const next = new URLSearchParams(params);
    // connection 上下文优先保留老 endpointId（除非调用方已显式给出）。
    if (!next.get("connection") && endpointId) next.set("connection", endpointId);
    const qs = next.toString();
    nav(`/devices/${deviceId}/overview${qs ? `?${qs}` : ""}`, { replace: true });
  }, [deviceId, endpointId, params, nav]);

  return null;
}
