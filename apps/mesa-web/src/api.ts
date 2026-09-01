// 通用 API 客户端：仅依赖 Descriptor 契约，不含协议分支
const BASE = "";

async function getJson(path: string) {
  const r = await fetch(`${BASE}${path}`);
  if (!r.ok) throw new Error(`${r.status} ${path}`);
  return r.json();
}

async function postJson(path: string, body: unknown) {
  const r = await fetch(`${BASE}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  const j = await r.json().catch(() => ({}));
  return { status: r.status, body: j };
}

export const api = {
  listDrivers: () => getJson("/api/v1/drivers"),
  getDriver: (id: string) => getJson(`/api/v1/drivers/${id}`),
  getDescriptor: (id: string) => getJson(`/api/v1/drivers/${id}/descriptor`),
  listProfiles: () => getJson("/api/v1/profiles"),
  getProfile: (id: string) => getJson(`/api/v1/profiles/${id}`),
  validateConnection: (id: string, connection: unknown) =>
    postJson(`/api/v1/drivers/${id}/validate-connection`, { connection }),
  probe: (id: string, connection: unknown) =>
    postJson(`/api/v1/drivers/${id}/probe`, { connection }),
  listEndpoints: () => getJson("/api/v1/endpoints"),
  listDevices: () => getJson("/api/v1/devices"),
  diagnostics: () => getJson("/api/v1/diagnostics"),
  endpointDiagnostics: (id: string) => getJson(`/api/v1/endpoints/${id}/diagnostics`),
};
