import { describe, expect, it } from "vitest";

import {
  classifyProbeOutcome,
  containsSecretMarker,
  describeIssue,
  describeOutcome,
  groupFields,
  initialSetupState,
  isBooleanField,
  isDurationField,
  isIntegerField,
  isFieldVisible,
  isResultCurrent,
  isSecretField,
  isSecretMarker,
  missingRequiredAdvanced,
  networkFailureOutcome,
  normalizeConnection,
  outcomeTone,
  parseIntInput,
  preserveHiddenFields,
  safeText,
  secretMarkerKeys,
  secretUnavailableOutcome,
  submitBlockReason,
  withDriverChange,
  withFieldChange,
  type FieldDescriptor,
  type SubmitGateInput,
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

describe("预设分级（V1.1 §5.2/§5.3）", () => {  it("单台 828D 实测值只能是 site_tested_example，不得自动预填", () => {
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

// ---------------------------------------------------------------------------
// 字段类型判定：wire 形态是小写（core-types `#[serde(rename_all="lowercase")]`）
// ---------------------------------------------------------------------------

describe("条件可见字段（ui.visible_if）", () => {
  const fields: FieldDescriptor[] = [
    { key: "use_native", label: "Use native", field_type: "boolean", required: false, default: false, ui: { order: 1 } },
    {
      key: "native_pdu",
      label: "PDU",
      field_type: "integer",
      required: true,
      default: 8192,
      validation: { min: 512, max: 8192 },
      ui: { order: 10, advanced: true, visible_if: { field: "use_native", op: "eq", value: true } },
    },
  ];

  it("条件不满足时：既不提交该字段，也不报必填（否则用户永远无法提交）", () => {
    const { connection, issues } = normalizeConnection({ use_native: false }, fields);
    expect(issues).toEqual([]);
    expect(connection.use_native).toBe(false);
    expect("native_pdu" in connection).toBe(false);
    expect(missingRequiredAdvanced(fields, { use_native: false })).toEqual([]);
  });

  it("条件满足时：按正常规则校验与提交", () => {
    const ok = normalizeConnection({ use_native: true, native_pdu: "4096" }, fields);
    expect(ok.issues).toEqual([]);
    expect(ok.connection.native_pdu).toBe(4096);
    expect(missingRequiredAdvanced(fields, { use_native: true })).toEqual([]);

    // 可见且缺失 → 必须报必填并出现在「高级缺失」里（触发自动展开）
    const missing = normalizeConnection({ use_native: true }, fields);
    expect(missing.connection.native_pdu).toBe(8192); // 有默认值则物化
    expect(missingRequiredAdvanced(fields, { use_native: true, native_pdu: "" })).toEqual([
      "native_pdu",
    ]);
  });

  it("isFieldVisible 与 DescriptorFields 的可见性判定同源（eq/neq/in/无条件）", () => {
    expect(isFieldVisible(fields[1], { use_native: true })).toBe(true);
    expect(isFieldVisible(fields[1], { use_native: false })).toBe(false);
    expect(isFieldVisible(fields[0], {})).toBe(true);
    const neq: FieldDescriptor = { key: "x", ui: { visible_if: { field: "mode", op: "neq", value: "a" } } };
    expect(isFieldVisible(neq, { mode: "b" })).toBe(true);
    expect(isFieldVisible(neq, { mode: "a" })).toBe(false);
    const inOp: FieldDescriptor = { key: "y", ui: { visible_if: { field: "mode", op: "in", value: ["a", "b"] } } };
    expect(isFieldVisible(inOp, { mode: "b" })).toBe(true);
    expect(isFieldVisible(inOp, { mode: "c" })).toBe(false);
  });
});

describe("字段类型判定（wire 小写）", () => {  it("小写 integer/port/duration 必须做范围校验（不能因为大小写漏掉）", () => {
    expect(isIntegerField("integer")).toBe(true);
    expect(isIntegerField("port")).toBe(true);
    expect(isIntegerField("duration")).toBe(true);
    // 历史 PascalCase 写法继续容忍
    expect(isIntegerField("Integer")).toBe(true);
    expect(isIntegerField("string")).toBe(false);
    expect(isBooleanField("boolean")).toBe(true);
    expect(isSecretField("secret")).toBe(true);

    const fields: FieldDescriptor[] = [
      { key: "port", label: "Port", field_type: "port", required: true, validation: { min: 1, max: 65535 } },
      { key: "timeout_ms", label: "Timeout", field_type: "duration", required: true, validation: { min: 500, max: 10000 } },
    ];
    const { issues } = normalizeConnection({ port: 70000, timeout_ms: 10 }, fields);
    expect(issues.map((i) => i.code).sort()).toEqual(["OUT_OF_RANGE", "OUT_OF_RANGE"]);
  });

  it("小写 boolean 必须为布尔值（字符串 true 不得混过）", () => {
    const fields: FieldDescriptor[] = [
      { key: "use_native", label: "Use native", field_type: "boolean", required: false },
    ];
    expect(normalizeConnection({ use_native: true }, fields).issues).toEqual([]);
    expect(normalizeConnection({ use_native: "true" }, fields).issues[0].code).toBe("NOT_BOOLEAN");
  });

  it("小写 secret 原样透传：marker 保留，供保存时「不更新凭据」", () => {
    const fields: FieldDescriptor[] = [
      { key: "api_token", label: "Token", field_type: "secret", required: false },
    ];
    const { connection, issues } = normalizeConnection(
      { api_token: { secret_set: true } },
      fields,
    );
    expect(issues).toEqual([]);
    expect(connection.api_token).toEqual({ secret_set: true });
  });
});

// ---------------------------------------------------------------------------
// Secret-safe 边界（V1.1 §9.4 / C2 约束）
// ---------------------------------------------------------------------------

describe("Secret marker 形状识别与阻断集合", () => {
  it("isSecretMarker 只认 marker 形状，不把明文/普通对象误判", () => {
    expect(isSecretMarker({ secret_set: true })).toBe(true);
    expect(isSecretMarker({ clear_secret: true })).toBe(true);
    expect(isSecretMarker("plain-password")).toBe(false);
    expect(isSecretMarker({ other: 1 })).toBe(false);
    expect(isSecretMarker(null)).toBe(false);
    expect(isSecretMarker([{ secret_set: true }])).toBe(false);
  });

  it("containsSecretMarker 递归识别嵌套 marker", () => {
    expect(containsSecretMarker({ auth: { password: { secret_set: true } } })).toBe(true);
    expect(containsSecretMarker([{ x: 1 }, { clear_secret: true }])).toBe(true);
    expect(containsSecretMarker({ auth: { password: "plain" } })).toBe(false);
  });

  it("按**形状**命中，不依赖字段名（api_token 也必须被识别）", () => {
    const fields: FieldDescriptor[] = [
      { key: "host", label: "Host", field_type: "host", required: true },
      { key: "api_token", label: "Token", field_type: "secret", required: false },
    ];
    expect(secretMarkerKeys(fields, { host: "h", api_token: { secret_set: true } })).toEqual([
      "api_token",
    ]);
  });

  it("字段类型未声明为 secret，但值仍是 marker → 同样命中（形状优先）", () => {
    const fields: FieldDescriptor[] = [
      { key: "credentials", label: "Credentials", field_type: "string", required: false },
    ];
    expect(secretMarkerKeys(fields, { credentials: { secret_set: true } })).toEqual([
      "credentials",
    ]);
  });

  it("Secret 字段出现无法识别的对象值 → 保守阻断（对象不可能是密码）", () => {
    const fields: FieldDescriptor[] = [
      { key: "password", label: "Password", field_type: "secret", required: false },
    ];
    expect(secretMarkerKeys(fields, { password: { something: "new" } })).toEqual(["password"]);
  });

  it("用户重新输入明文后不再命中（测试随之解禁）", () => {
    const fields: FieldDescriptor[] = [
      { key: "password", label: "Password", field_type: "secret", required: false },
    ];
    expect(secretMarkerKeys(fields, { password: "new-secret" })).toEqual([]);
    expect(secretMarkerKeys(fields, {})).toEqual([]);
  });

  it("secret_unavailable 是独立 outcome：不冒充设备不可达，也不冒充驱动不支持", () => {
    const outcome = secretUnavailableOutcome("凭据未重新输入");
    expect(outcome.kind).toBe("secret_unavailable");
    const text = describeOutcome(outcome);
    expect(text).toContain("未执行探测");
    expect(text).toContain("凭据未重新输入");
    expect(text).not.toContain("设备不可达");
    expect(text).not.toContain("不支持此探测");
    expect(outcomeTone(outcome)).toBe("warning");
  });
});

// ---------------------------------------------------------------------------
// P0：提交前置门（按钮 disabled 与提交函数守卫共用同一判定）
// ---------------------------------------------------------------------------

describe("提交前置门 submitBlockReason（P0）", () => {
  const ready: SubmitGateInput = {
    active: true,
    driverSelectable: true,
    driversStatus: "ready",
    usableDriverIds: ["s7"],
    driverId: "s7",
    descriptorStatus: "ready",
  };

  it("只有「面板可见 + 清单就绪 + 驱动可用 + 描述符就绪」才放行", () => {
    expect(submitBlockReason(ready)).toBeNull();
  });

  it("清单失败/未就绪一律挡住（不回退到内置驱动列表）", () => {
    for (const status of ["idle", "loading", "error"] as const) {
      expect(submitBlockReason({ ...ready, driversStatus: status })).toBe("catalog-unavailable");
    }
  });

  it("无可用驱动（含选中的驱动不在 launchable 集合里）一律挡住", () => {
    expect(submitBlockReason({ ...ready, usableDriverIds: [] })).toBe("no-usable-driver");
    expect(submitBlockReason({ ...ready, driverId: null })).toBe("no-usable-driver");
    expect(
      submitBlockReason({ ...ready, driverId: "dead", usableDriverIds: ["s7"] }),
    ).toBe("no-usable-driver");
  });

  it("描述符未就绪/失败一律挡住", () => {
    for (const status of ["idle", "loading", "error"] as const) {
      expect(submitBlockReason({ ...ready, descriptorStatus: status })).toBe(
        "descriptor-unavailable",
      );
    }
  });

  it("面板不可见时一律挡住（弹窗关闭 / 非当前步骤不得发请求）", () => {
    expect(submitBlockReason({ ...ready, active: false })).toBe("inactive");
  });

  it("edit 模式（driverSelectable=false）不看清单，但描述符仍必须就绪", () => {
    const edit: SubmitGateInput = {
      active: true,
      driverSelectable: false,
      driversStatus: "idle",
      usableDriverIds: [],
      driverId: "opcua",
      descriptorStatus: "ready",
    };
    expect(submitBlockReason(edit)).toBeNull();
    expect(submitBlockReason({ ...edit, descriptorStatus: "error" })).toBe(
      "descriptor-unavailable",
    );
    expect(submitBlockReason({ ...edit, driverId: null })).toBe("no-driver");
  });
});

// ---------------------------------------------------------------------------
// P1：编辑态 visible_if 隐藏字段不得丢失（后端整体替换 connection）
// ---------------------------------------------------------------------------

describe("编辑态隐藏字段保留 preserveHiddenFields（P1）", () => {
  const FIELDS: FieldDescriptor[] = [
    { key: "host", label: "Host", field_type: "host", required: true },
    { key: "use_native", label: "Use native", field_type: "boolean", default: false },
    {
      key: "native_pdu",
      label: "PDU",
      field_type: "integer",
      validation: { min: 1, max: 65535 },
      ui: { visible_if: { field: "use_native", op: "eq", value: true } },
    },
    { key: "api_token", label: "Token", field_type: "secret" },
  ];

  it("正向：visible_if 不满足的字段保留服务端快照原值", () => {
    const raw = { host: "10.0.0.2", use_native: false, native_pdu: 8192 };
    const canonical = normalizeConnection(raw, FIELDS);
    // 前提：归一化确实把隐藏字段排除了（否则本测试无意义）
    expect("native_pdu" in canonical.connection).toBe(false);

    const out = preserveHiddenFields(canonical.connection, raw, FIELDS);
    expect(out.native_pdu).toBe(8192);
    expect(out.host).toBe("10.0.0.2");
    expect(out.use_native).toBe(false);
  });

  it("负例：旧 connection 里不属于当前 Descriptor 的字段不得被带回", () => {
    const raw = {
      host: "10.0.0.2",
      use_native: false,
      native_pdu: 8192,
      legacy_field: "old",
      gone_object: { a: 1 },
    };
    const canonical = normalizeConnection(raw, FIELDS);
    const out = preserveHiddenFields(canonical.connection, raw, FIELDS);

    expect("legacy_field" in out).toBe(false);
    expect("gone_object" in out).toBe(false);
    // 精确断言：只可能是 Descriptor 内的字段（证明不是无脑整体合并）
    expect(Object.keys(out).sort()).toEqual(["host", "native_pdu", "use_native"]);
  });

  it("用户显式修改的值以用户输入为准，不被旧值覆盖", () => {
    // 用户在字段仍可见时把它改成 4096，随后切换条件使其隐藏
    const raw = { host: "10.0.0.2", use_native: false, native_pdu: 4096 };
    const canonical = normalizeConnection(raw, FIELDS);
    const out = preserveHiddenFields(canonical.connection, raw, FIELDS);
    expect(out.native_pdu).toBe(4096);
  });

  it("用户显式清除（null/空串）以用户输入为准，不得恢复旧值", () => {
    const raw = { host: "10.0.0.2", use_native: false, native_pdu: null };
    const canonical = normalizeConnection(raw, FIELDS);
    const out = preserveHiddenFields(canonical.connection, raw, FIELDS);
    // 保留的是用户的「空」，随后 cleanConnection 会把它从请求体剔除 = 删除该字段
    expect(out.native_pdu).toBeNull();
    expect(out.native_pdu).not.toBe(8192);
  });

  it("可见字段缺席是校验问题：不得用旧值掩盖", () => {
    // native_pdu 可见但值非法 → 归一化产生 issue 且不写入 canonical
    const raw = { host: "10.0.0.2", use_native: true, native_pdu: "abc" };
    const canonical = normalizeConnection(raw, FIELDS);
    expect(canonical.issues.map((i) => i.code)).toContain("NOT_INTEGER");

    const out = preserveHiddenFields(canonical.connection, raw, FIELDS);
    expect("native_pdu" in out).toBe(false);
  });

  it("raw 里没有的 key 不凭空补值（不替 Descriptor 发明默认值）", () => {
    const raw = { host: "10.0.0.2", use_native: false };
    const canonical = normalizeConnection(raw, FIELDS);
    const out = preserveHiddenFields(canonical.connection, raw, FIELDS);
    expect("native_pdu" in out).toBe(false);
  });

  it("Secret 的 marker 语义不受影响：原样随负载回传", () => {
    const raw = { host: "10.0.0.2", use_native: false, api_token: { secret_set: true } };
    const canonical = normalizeConnection(raw, FIELDS);
    const out = preserveHiddenFields(canonical.connection, raw, FIELDS);
    expect(out.api_token).toEqual({ secret_set: true });
    // 清除标记同样原样透传
    const cleared = { host: "10.0.0.2", use_native: false, api_token: { clear_secret: true } };
    const canonical2 = normalizeConnection(cleared, FIELDS);
    expect(preserveHiddenFields(canonical2.connection, cleared, FIELDS).api_token).toEqual({
      clear_secret: true,
    });
  });
});

// ---------------------------------------------------------------------------
// P1 附带：duration / integer / port 的类型语义一致性
//
// 契约核查（以代码为准）：
// - Core Schema：`Number | Duration => val.is_number()`（duration 允许小数、无上界）；
// - Driver Parser：`timeout_ms` 一律 `as_u64()`（要求非负整数，小数报 BAD_CONFIG）；
// - 前端：与 Driver 对齐（拒绝小数），并去掉「整数/端口 65535 兜底」对 duration 的误用。
// ---------------------------------------------------------------------------

describe("duration 与 integer/port 的类型语义一致性", () => {
  const fields: FieldDescriptor[] = [
    { key: "n", label: "N", field_type: "integer", required: true, validation: { min: 0, max: 65535 } },
    { key: "port", label: "Port", field_type: "port", required: true, validation: { min: 1, max: 65535 } },
    { key: "timeout_ms", label: "Timeout", field_type: "duration", required: true, validation: { min: 500, max: 10000 } },
  ];

  it("duration 与 integer/port 一样拒绝小数（Driver Parser 用 as_u64() 读取）", () => {
    expect(isDurationField("duration")).toBe(true);
    expect(isDurationField("Duration")).toBe(true);
    expect(isDurationField("integer")).toBe(false);

    const cases: Array<Record<string, unknown>> = [
      { n: 1.5, port: 102, timeout_ms: 5000 },
      { n: 1, port: 102.5, timeout_ms: 5000 },
      { n: 1, port: 102, timeout_ms: 5000.5 },
      { n: 1, port: 102, timeout_ms: "5000.5" },
    ];
    for (const raw of cases) {
      const { issues } = normalizeConnection(raw, fields);
      expect(issues.map((i) => i.code)).toEqual(["NOT_INTEGER"]);
    }
    // 整数形态照常通过
    expect(normalizeConnection({ n: 1, port: 102, timeout_ms: 5000 }, fields).issues).toEqual([]);
  });

  it("duration 没有 16 位上界：未声明 max 时不得用 65535 兜底（前端拒绝/后端接受的矛盾）", () => {
    const dur: FieldDescriptor[] = [
      { key: "timeout_ms", label: "Timeout", field_type: "duration", required: false },
    ];
    expect(normalizeConnection({ timeout_ms: 60000 }, dur)).toEqual({
      connection: { timeout_ms: 60000 },
      issues: [],
    });
    // 负数仍被拒：Core Schema 对 Duration 的内禀范围是 >= 0
    expect(normalizeConnection({ timeout_ms: -1 }, dur).issues[0].code).toBe("OUT_OF_RANGE");
    // integer/port 沿用既有 65535 兜底，行为不变
    const int: FieldDescriptor[] = [
      { key: "n", label: "N", field_type: "integer", required: false },
    ];
    expect(normalizeConnection({ n: 70000 }, int).issues[0].code).toBe("OUT_OF_RANGE");
    // Descriptor 显式声明的 min/max 仍然生效（duration 也不例外）
    const bounded: FieldDescriptor[] = [
      { key: "timeout_ms", label: "T", field_type: "duration", required: false, validation: { min: 500, max: 10000 } },
    ];
    expect(normalizeConnection({ timeout_ms: 10001 }, bounded).issues[0].code).toBe("OUT_OF_RANGE");
    expect(normalizeConnection({ timeout_ms: 499 }, bounded).issues[0].code).toBe("OUT_OF_RANGE");
  });
});

// ---------------------------------------------------------------------------
// P2：safeText 对带 path 的 issue 不得重复前缀
// ---------------------------------------------------------------------------

describe("safeText：issue path 前缀去重（P2）", () => {
  it("message 已自带同一 path 前缀时裁掉重复段", () => {
    expect(
      safeText({ path: "connection.host", code: "REQUIRED", message: "connection.host 缺少必填" }),
    ).toBe("connection.host: REQUIRED: 缺少必填");
    expect(safeText({ path: "connection.host", code: "REQUIRED", message: "connection.host" })).toBe(
      "connection.host: REQUIRED",
    );
    expect(
      safeText({ path: "connection.host", code: "REQUIRED", message: "connection.host：缺少必填" }),
    ).toBe("connection.host: REQUIRED: 缺少必填");
  });

  it("只是恰好同前缀的另一个词不得误裁（向后兼容）", () => {
    expect(
      safeText({ path: "connection.host", code: "C", message: "connection.hostname 非法" }),
    ).toBe("connection.host: C: connection.hostname 非法");
    // 不含前缀时输出逐字不变
    expect(safeText({ path: "connection.host", code: "REQUIRED", message: "缺少必填" })).toBe(
      "connection.host: REQUIRED: 缺少必填",
    );
    expect(safeText({ code: "C", message: "M" })).toBe("C: M");
  });

  it("describeIssue：path 与 message 分开渲染的那一处同样不重复前缀", () => {
    // 重复前缀真正发生的位置：渲染层把 path 与 message 拼成一行
    expect(
      describeIssue({
        path: "connection.host",
        code: "REQUIRED",
        message: "connection.host 缺少必填",
      }),
    ).toBe("connection.host：缺少必填（REQUIRED）");
    // 不含重复时输出与既有格式逐字一致（向后兼容）
    expect(
      describeIssue({ path: "connection.host", code: "REQUIRED", message: "缺少必填" }),
    ).toBe("connection.host：缺少必填（REQUIRED）");
    // code 缺失时不渲染空括号
    expect(describeIssue({ path: "connection.host", code: "", message: "缺少必填" })).toBe(
      "connection.host：缺少必填",
    );
  });
});
