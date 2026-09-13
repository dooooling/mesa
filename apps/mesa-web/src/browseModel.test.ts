// Browse → Selection 映射单测：只锁定两种信封形态，不解释协议语义。
import { describe, expect, it } from "vitest";
import { buildBrowseRequest, selectableFromNode } from "./browseModel";

describe("selectableFromNode", () => {
  it("NCK 形态：显式 resource_id + parameters", () => {
    const sel = selectableFromNode({
      id: "nck://C/1/SEMA/axConf[1]",
      kind: "variable",
      has_children: false,
      binding_json: JSON.stringify({
        resource_id: "variable",
        parameters: { area: "C", block: "SEMA", variable: "axConf", count: 1 },
      }),
    });
    expect(sel).toEqual({
      resource_id: "variable",
      parameters: { area: "C", block: "SEMA", variable: "axConf", count: 1 },
    });
  });

  it("OPC UA 形态：裸参数 + kind 回落为 resource", () => {
    const sel = selectableFromNode({
      id: "nsu=urn:mesa:fake:1;i=1",
      kind: "node",
      has_children: true,
      binding_json: JSON.stringify({ node_id: "nsu=urn:mesa:fake:1;i=1", data_type: "STRING" }),
    });
    expect(sel).toEqual({
      resource_id: "node",
      parameters: { node_id: "nsu=urn:mesa:fake:1;i=1", data_type: "STRING" },
    });
  });

  it("无 binding 的分支节点不可选用（只能下钻）", () => {
    expect(selectableFromNode({ id: "nck://C", kind: "group", has_children: true })).toBeNull();
    expect(selectableFromNode({ id: "x", kind: "node", binding_json: "  " })).toBeNull();
  });

  it("非法 JSON / 非对象 binding 不可用", () => {
    expect(selectableFromNode({ id: "x", kind: "node", binding_json: "{oops" })).toBeNull();
    expect(selectableFromNode({ id: "x", kind: "node", binding_json: "[1,2]" })).toBeNull();
    expect(selectableFromNode({ id: "x", kind: "node", binding_json: '"str"' })).toBeNull();
  });

  it("无 resource_id 又无 kind 时不可用", () => {
    expect(selectableFromNode({ id: "x", binding_json: JSON.stringify({ a: 1 }) })).toBeNull();
  });
});

describe("buildBrowseRequest", () => {
  it("缺省 parent/filter/cursor/limit", () => {
    expect(buildBrowseRequest({})).toEqual({ parent: "", filter: "", cursor: "", limit: 50 });
  });
});
