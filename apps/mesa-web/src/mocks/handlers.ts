// msw mock（F 前仅静态，F 后可联调后端，§23 503+error.code形态）
import { http, HttpResponse } from "msw";
import type { DriverDescriptor } from "../types";

// mock descriptor 受 DriverDescriptor 约束（satisfies）：Rust/TS wire 漂移即红
const mockDescriptor = {
  contract_major: 2,
  contract_minor: 0,
  identity: { driver_id: "mock", name: "Mock", version: "0.0.0" },
  connection: {
    fields: [
      {
        key: "host",
        label: "主机",
        field_type: "host",
        required: true,
        validation: {},
        ui: {},
      },
    ],
  },
  resources: [
    {
      id: "counter",
      label: { default: "计数器" },
      parameters: { fields: [] },
      outputs: [{ id: "value", label: { default: "值" }, type_spec: { kind: "fixed", data_type: "F64" }, access: "read" }],
      modes: ["poll"],
    },
  ],
  events: { streams: [] },
  controls: { commands: [{ id: "reset", label: { default: "复位" }, risk: "low" }] },
  resource_selection_methods: ["manual"],
  capabilities: { poll: true, subscribe: false, write: false, method: false, events: false },
} satisfies DriverDescriptor;

export const handlers = [
  http.get("/api/v1/drivers/:id/descriptor", ({ params }) =>
    HttpResponse.json({
      ...mockDescriptor,
      identity: { driver_id: params.id, name: `Mock ${params.id}`, version: "0.0.0" },
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
