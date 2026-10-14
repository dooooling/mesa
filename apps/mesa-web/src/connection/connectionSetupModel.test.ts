import { describe, expect, it } from "vitest";

import {
  classifyProbeOutcome,
  describeOutcome,
  groupFields,
  initialSetupState,
  isResultCurrent,
  missingRequiredAdvanced,
  networkFailureOutcome,
  normalizeConnection,
  outcomeTone,
  parseIntInput,
  safeText,
  withDriverChange,
  withFieldChange,
  type FieldDescriptor,
} from "./connectionSetupModel";
import {
  autoApplicablePreset,
  fieldsToClearOnSwitch,
  NCK_828D_SITE_TESTED,
  selectablePresets,
} from "./connectionProfiles";

/** 一个贴近真实 Descriptor 的最小字段集（NCK 形态）。 */
const NCK_FIELDS: FieldDescriptor[] = [
  { key: "host", label: "Host", field_type: "Host", required: true, ui: { order: 1 } },
  { key: "port", label: "Port", field_type: "Port", required: false, default: 102, validation: { min: 1, max: 65535 }, ui: { order: 2 } },
  { key: "family", label: "Family", field_type: "Enum", required: true, validation: { enum_options: ["828d", "840d-sl"] }, ui: { order: 3 } },
  { key: "local_tsap", label: "Local TSAP", field_type: "Integer", required: true, validation: { min: 0, max: 65535 }, ui: { order: 10, advanced: true } },
  { key: "remote_tsap", label: "Remote TSAP", field_type: "Integer", required: true, validation: { min: 0, max: 65535 }, ui: { order: 11, advanced: true } },
  { key: "timeout_ms", label: "Timeout ms", field_type: "Duration", required: false, default: 5000, validation: { min: 500, max: 10000 }, ui: { order: 12, advanced: true } },
];

const S7_FIELDS: FieldDescriptor[] = [
  { key: "host", label: "Host", field_type: "Host", required: true, ui: { order: 1 } },
  { key: "port", label: "Port", field_type: "Port", required: false, default: 102, ui: { order: 2 } },
  { key: "rack", label: "Rack", field_type: "Integer", required: false, default: 0, ui: { order: 3 } },
  { key: "slot", label: "Slot", field_type: "Integer", required: false, default: 1, ui: { order: 4 } },
  { key: "timeout_ms", label: "Timeout ms", field_type: "Duration", required: false, default: 3000, ui: { order: 10, advanced: true } },
];

describe("parseIntInput（TSAP / 整数文本）", () => {
  it("A06：TSAP 的十六进制与十进制等价", () => {
    for (const raw of ["1024", "0x0400", "0X0400", "0400h", "0400H", " 0x0400 "]) {
      expect(parseIntInput(raw, { allowHex: true })).toEqual({ ok: true, value: 1024 });
    }
  });

  it("A05：非法 TSAP 输入不得回退为上一次合法值，必须给出 issue", () => {
    for (const raw of ["", "  ", "0x", "0xZZ", "0x10000", "12345h", "12.5", "abc", true]) {
      const r = parseIntInput(raw, { allowHex: true });
      expect(r.ok).toBe(false);
    }
  });

  it("非 TSAP 的整数字段不接受十六进制（避免把 0x0100 当成另一个数）", () => {
    expect(parseIntInput("0x0400", { allowHex: false }).ok).toBe(false);
    expect(parseIntInput("1024", { allowHex: false })).toEqual({ ok: true, value: 1024 });
  });

  it("范围检查（下限/上限/越界）", () => {
    expect(parseIntInput("499", { allowHex: false, min: 500, max: 10000 }).ok).toBe(false);
    expect(parseIntInput("10001", { allowHex: false, min: 500, max: 10000 }).ok).toBe(false);
    expect(parseIntInput("500", { allowHex: false, min: 500, max: 10000 })).toEqual({ ok: true, value: 500 });
    expect(parseIntInput("10000", { allowHex: false, min: 500, max: 10000 })).toEqual({ ok: true, value: 10000 });
  });
});

describe("normalizeConnection（raw draft → canonical）", () => {
  it("A05：未失焦的 raw 文本就是权威来源，归一化后提交的是最新值", () => {
    // 用户把 0x0400 改成 0x0500 但还没 blur
    const raw = { host: "10.0.0.5", family: "828d", local_tsap: "0x0500", remote_tsap: 3332 };
    const { connection, issues } = normalizeConnection(raw, NCK_FIELDS);
    expect(issues).toEqual([]);
    expect(connection.local_tsap).toBe(1280); // 0x0500
    expect(connection.remote_tsap).toBe(3332);
    expect(connection.port).toBe(102); // 来自 default
  });

  it("非法 raw 值产生 issue 且不写入 canonical（不得静默替换）", () => {
    const raw = { host: "10.0.0.5", family: "828d", local_tsap: "0x", remote_tsap: 3332 };
    const { connection, issues } = normalizeConnection(raw, NCK_FIELDS);
    expect(connection.local_tsap).toBeUndefined();
    expect(issues.map((i) => i.path)).toContain("connection.local_tsap");
  });

  it("A04：高级必填缺失能被定位（用于自动展开）", () => {
    const raw = { host: "10.0.0.5", family: "828d" };
    const missing = missingRequiredAdvanced(NCK_FIELDS, raw);
    expect(missing).toEqual(["local_tsap", "remote_tsap"]);
    const { issues } = normalizeConnection(raw, NCK_FIELDS);
    expect(issues.filter((i) => i.code === "REQUIRED").map((i) => i.path)).toEqual([
      "connection.local_tsap",
      "connection.remote_tsap",
    ]);
  });

  it("缺少无默认值的必填字段必须报错，不自动填充", () => {
    const { connection, issues } = normalizeConnection({}, NCK_FIELDS);
    expect(connection.host).toBeUndefined();
    expect(issues.some((i) => i.path === "connection.host" && i.code === "REQUIRED")).toBe(true);
  });

  it("Enum 非法取值被拒绝", () => {
    const { issues } = normalizeConnection(
      { host: "h", family: "840d", local_tsap: 1, remote_tsap: 2 },
      NCK_FIELDS,
    );
    expect(issues.some((i) => i.path === "connection.family" && i.code === "BAD_ENUM")).toBe(true);
  });

  it("字段分组来自 Descriptor，不在组件里硬编码", () => {
    const g = groupFields(NCK_FIELDS);
    expect(g.basic.map((f) => f.key)).toEqual(["host", "port", "family"]);
    expect(g.advanced.map((f) => f.key)).toEqual(["local_tsap", "remote_tsap", "timeout_ms"]);
  });
});

describe("状态机与代际（A01/A02/A03）", () => {
  it("A03：任一字段变化都会 revision+1 并让旧校验/探测结果失效", () => {
    let s = initialSetupState();
    s = withDriverChange(s, "sinumerik-nck", NCK_FIELDS);
    const rev0 = s.draft.revision;
    s = { ...s, lastValidatedRevision: rev0, probeRevision: rev0 };

    s = withFieldChange(s, "host", "10.0.0.9");
    expect(s.draft.revision).toBe(rev0 + 1);
    expect(s.lastValidatedRevision).toBeNull();
    expect(s.probeRevision).toBeNull();
  });

  it("A03：迟到的旧结果按 (driverId, revision) 判定为过期", () => {
    let s = initialSetupState();
    s = withDriverChange(s, "sinumerik-nck", NCK_FIELDS);
    const stale = { driverId: "sinumerik-nck", revision: s.draft.revision };
    expect(isResultCurrent(s, stale)).toBe(true);
    s = withFieldChange(s, "host", "10.0.0.9");
    expect(isResultCurrent(s, stale)).toBe(false);
    expect(isResultCurrent(s, { ...stale, driverId: "s7" })).toBe(false);
  });

  it("A01/A02：切换驱动按新 Schema 重建参数，不残留旧驱动字段", () => {
    let s = initialSetupState();
    s = withDriverChange(s, "sinumerik-nck", NCK_FIELDS);
    s = withFieldChange(s, "local_tsap", 1024);
    expect(s.draft.raw.local_tsap).toBe(1024);

    s = withDriverChange(s, "s7", S7_FIELDS);
    expect(s.driverId).toBe("s7");
    // 旧驱动的 TSAP 不得残留
    expect(s.draft.raw.local_tsap).toBeUndefined();
    expect(s.draft.raw.remote_tsap).toBeUndefined();
    // 新驱动只带自己的默认值
    expect(s.draft.raw).toEqual({ port: 102, rack: 0, slot: 1, timeout_ms: 3000 });
  });
});

describe("探测结果分类与展示（A09/A10/A11/A26）", () => {
  it("A11：各 HTTP 状态映射到不同 outcome，不把服务端错误说成 CNC 不可达", () => {
    expect(classifyProbeOutcome(200, { reachable: false }).kind).toBe("unreachable");
    expect(classifyProbeOutcome(400, { error: { code: "BAD_CONFIG", message: "x" } }).kind).toBe("invalid");
    expect(classifyProbeOutcome(404, {}).kind).toBe("not_found");
    expect(classifyProbeOutcome(501, {}).kind).toBe("unsupported");
    expect(classifyProbeOutcome(503, {}).kind).toBe("infrastructure");
    expect(classifyProbeOutcome(504, {}).kind).toBe("timeout");
    expect(classifyProbeOutcome(418, {}).kind).toBe("network");
  });

  it("A08：200 + reachable 但身份缺失 → 待确认（不得宣称已识别型号）", () => {
    const o = classifyProbeOutcome(200, {
      reachable: true,
      device: { vendor: null, family: null, model: null, firmware: null },
      warnings: [{ code: "NCK_ANCHOR_PENDING", message: "身份未确认" }],
    });
    expect(o.kind).toBe("reachable_unidentified");
    expect(describeOutcome(o)).toContain("身份未确认");
    expect(describeOutcome(o)).not.toContain("连接成功");
  });

  it("200 + 有身份 → 可达且返回身份信息", () => {
    const o = classifyProbeOutcome(200, {
      reachable: true,
      device: { vendor: "Siemens", family: "828d", model: null, firmware: null },
      warnings: [],
    });
    expect(o.kind).toBe("reachable_identified");
    expect(describeOutcome(o)).toContain("设备返回身份信息");
  });

  it("A26：结构化错误对象绝不被当作 React 文本渲染", () => {
    const o = classifyProbeOutcome(400, {
      error: { code: "REQUIRED", message: "field `module` is required" },
    });
    const text = describeOutcome(o);
    expect(typeof text).toBe("string");
    expect(text).toContain("REQUIRED");
    expect(text).toContain("field `module` is required");
    // 关键：不得出现 "[object Object]"
    expect(text).not.toContain("[object Object]");
  });

  it("safeText 对嵌套/未知形态都返回字符串", () => {
    expect(safeText(undefined)).toBe("");
    expect(safeText({ a: 1 })).toBe('{"a":1}');
    expect(safeText({ code: "C", message: "M" })).toBe("C: M");
    expect(safeText([1, "x"])).toBe("1；x");
    expect(typeof safeText(Symbol.iterator as unknown)).toBe("string");
  });

  it("A10：501 允许保存合法配置（色调为 error 但语义是「不支持探测」）", () => {
    const o = classifyProbeOutcome(501, {});
    expect(describeOutcome(o)).toContain("不支持此探测");
    expect(outcomeTone(o)).toBe("error");
  });

  it("网络层失败与设备不可达区分", () => {
    const o = networkFailureOutcome(new Error("Failed to fetch"));
    expect(o.kind).toBe("network");
    expect(describeOutcome(o)).toContain("无法访问 Mesa API");
    expect(describeOutcome(o)).not.toContain("设备不可达");
  });

  it("色调：只有「可达且已识别」是 positive，避免误报绿色成功", () => {
    expect(outcomeTone(classifyProbeOutcome(200, { reachable: true, device: { vendor: "S" } }))).toBe("positive");
    expect(outcomeTone(classifyProbeOutcome(200, { reachable: true, device: {} }))).toBe("neutral");
    expect(outcomeTone(classifyProbeOutcome(200, { reachable: false }))).toBe("warning");
    expect(outcomeTone(classifyProbeOutcome(503, {}))).toBe("error");
  });
});

describe("预设分级（V1.1 §5.2/§5.3）", () => {
  it("单台 828D 实测值只能是 site_tested_example，不得自动预填", () => {
    expect(NCK_828D_SITE_TESTED.scope).toBe("site_tested_example");
    expect(autoApplicablePreset("sinumerik-nck", "828d")).toBeNull();
  });

  it("示例预设可被用户显式选择，且必须带来源与限制说明", () => {
    const list = selectablePresets("sinumerik-nck", "828d");
    expect(list).toHaveLength(1);
    expect(list[0].provenance).toContain("docs/evidence/");
    expect(list[0].caveat).toContain("不是所有 828D 的通用默认");
  });

  it("不提供 Host 预填（Host 必须由用户明确填写）", () => {
    expect(Object.keys(NCK_828D_SITE_TESTED.values)).not.toContain("host");
  });

  it("切换到不适用系列时清除不再适用的预填值", () => {
    expect(fieldsToClearOnSwitch(NCK_828D_SITE_TESTED, null)).toEqual([
      "local_tsap",
      "remote_tsap",
    ]);
    expect(fieldsToClearOnSwitch(NCK_828D_SITE_TESTED, NCK_828D_SITE_TESTED)).toEqual([]);
  });

  it("没有 family 时不返回任何预设（不跨系列猜测）", () => {
    expect(autoApplicablePreset("sinumerik-nck", null)).toBeNull();
    expect(selectablePresets("sinumerik-nck", null)).toEqual([]);
  });
});
