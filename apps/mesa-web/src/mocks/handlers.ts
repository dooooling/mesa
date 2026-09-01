// msw mock（F 前仅静态，F 后可联调后端，§23 503+error.code形态）
import { http, HttpResponse } from "msw";
export const handlers = [
  http.get("/api/v1/drivers/:id/descriptor", ({ params }) =>
    HttpResponse.json({
      contract_major: 1,
      contract_minor: 0,
      identity: { driver_id: params.id, name: `Mock ${params.id}`, version: "0.0.0" },
      connection: { fields: [{ key: "host", label: { zh: "主机" }, field_type: "Host", required: true }] },
      resources: [{ id: "counter", label: { zh: "计数器" }, parameters: { fields: [] }, outputs: [{ id: "value", label: { zh: "值" }, data_type: "F64" }], modes: ["poll"] }],
      controls: { commands: [{ id: "reset", label: { zh: "复位" }, risk: "low" }] },
      discovery: { manual: true }, capabilities: { poll: true }
    })
  ),
  http.post("/api/v1/drivers/:id/validate-connection", async () =>
    HttpResponse.json({ valid: true, issues: [] })
  ),
  http.post("/api/v1/drivers/:id/probe", async () =>
    HttpResponse.json({ reachable: true, warnings: [] })
  ),
  http.post("/api/v1/endpoints/:id/browse", async () =>
    HttpResponse.json({ nodes: [{ id: "n1", label: "Node1", kind: "node", data_type: "F64", access: "read", has_children: false, binding_json: "{}" }], next_cursor: "" })
  ),
  http.get("/api/v1/control/audit", () =>
    HttpResponse.json({ audits: [], next_cursor: null, count: 0 })
  ),
];
