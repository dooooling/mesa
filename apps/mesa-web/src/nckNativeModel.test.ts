// PR-C 第三批：nck_native 高级模式纯模型（A24–A27）。
import { describe, expect, it } from "vitest";
import {
  buildNativePointSelection,
  nativeAddressParameters,
  nativeDerivedPointKey,
  validateNativeFields,
  validateNativePoint,
  validateNativeUserPointKey,
} from "./nckNativeModel";
import type { SchemaDescriptor } from "./types";

const nativeValues = {
  area: "C",
  area_no: 1,
  module: 21,
  column: 1,
  line: 396,
  count: 1,
  unit_mode: "current",
  decode: "U16_LE",
  transport_size: "0x05",
  invalid_value: -200,
  invalid_tolerance: 0.001,
  unit: "mm",
};

const schema: SchemaDescriptor = {
  fields: [
    { key: "area", label: "Area", field_type: "enum", required: true, validation: { enum_options: ["N", "C"] }, ui: {} },
    { key: "area_no", label: "Area No.", field_type: "integer", required: true, validation: { min: 0, max: 31 }, ui: {} },
    { key: "module", label: "Module", field_type: "integer", required: true, validation: { min: 0, max: 255 }, ui: {} },
    { key: "column", label: "Column", field_type: "integer", required: true, validation: { min: 0, max: 65535 }, ui: {} },
    { key: "line", label: "Line", field_type: "integer", required: true, validation: { min: 0, max: 65535 }, ui: {} },
    { key: "count", label: "Count", field_type: "integer", required: true, default: 1, validation: { min: 1, max: 255 }, ui: {} },
    { key: "unit_mode", label: "Unit mode", field_type: "enum", required: true, default: "current", validation: { enum_options: ["current", "metric"] }, ui: {} },
    { key: "decode", label: "Decode", field_type: "enum", required: true, validation: { enum_options: ["U16_LE"] }, ui: {} },
    { key: "transport_size", label: "Transport", field_type: "enum", required: true, validation: { enum_options: ["0x05"] }, ui: {} },
  ],
};

describe("A27：derived Key 是冻结的固定规则", () => {
  it("native_derived_key_uses_area_area_no_module_column_line_only", () => {
    expect(nativeDerivedPointKey(nativeValues)).toBe("native.C.1.21.1.396");
    // count/decode/transport_size/invalid_* 不参与身份。
    expect(
      nativeDerivedPointKey({
        area: "C",
        area_no: 1,
        module: 21,
        column: 1,
        line: 396,
        count: 99,
        decode: "F32_BE",
      }),
    ).toBe("native.C.1.21.1.396");
    expect(nativeDerivedPointKey({ area: "C", area_no: 1, module: 21, column: 1 })).toBeNull();
    expect(nativeDerivedPointKey({ area: "1", area_no: 1, module: 21, column: 1, line: 1 })).toBeNull();
    expect(nativeDerivedPointKey({ area: "C", area_no: 1.5, module: 21, column: 1, line: 1 })).toBeNull();
  });
});

describe("A25/A26：user Key 显式、保持原值、命名空间/语法严格", () => {
  it("valid_user_keys_are_accepted_without_rename", () => {
    expect(validateNativeUserPointKey("native.C.1.21.1.396")).toEqual({
      ok: true,
      key: "native.C.1.21.1.396",
    });
    // 允许 Core 支持的 ~<n>（n>=2），但**不自动生成**。
    expect(validateNativeUserPointKey("native.C.1.21.1.396~2").ok).toBe(true);
    for (const bad of [
      "channel.program.status@channel=i:1",
      "native.C.1.21.1",
      "native.C.01.21.1.396",
      "native.1.1.21.1.396",
      "native.C.1.21.1.396~1",
      "native.C.1.21.1.396~02",
      "native.C.1.21.1.396~",
      " native.C.1.21.1.396",
      "native.C.1.21.1.396 ",
    ]) {
      const check = validateNativeUserPointKey(bad);
      expect(check.ok, bad).toBe(false);
      if (!check.ok) expect(check.code).toBe("NATIVE_KEY_INVALID");
    }
  });

  it("build_user_point_preserves_key_and_marks_identity_mode", () => {
    const built = buildNativePointSelection({
      values: nativeValues,
      identityMode: "user",
      userKey: "native.C.1.21.1.396~2",
    });
    expect(built.ok).toBe(true);
    if (built.ok) {
      // 不做任何自动改名/追加后缀。
      expect(built.point.point_key).toBe("native.C.1.21.1.396~2");
      expect(built.point.parameters.identity_mode).toBe("user");
    }
  });

  it("build_derived_point_omits_identity_mode", () => {
    const built = buildNativePointSelection({ values: nativeValues, identityMode: "derived" });
    expect(built.ok).toBe(true);
    if (built.ok) {
      expect(built.point.point_key).toBe("native.C.1.21.1.396");
      expect(built.point.parameters).not.toHaveProperty("identity_mode");
      expect(built.point.parameters).not.toHaveProperty("area_no_extra");
    }
    expect(nativeAddressParameters({ ...nativeValues, identity_mode: "user" })).not.toHaveProperty(
      "identity_mode",
    );
  });
});

describe("原生字段按 Descriptor Schema 校验（不做协议推断）", () => {
  it("required/range/enum violations_are_reported", () => {
    expect(validateNativeFields(schema, nativeValues)).toEqual([]);
    const missing = validateNativeFields(schema, { area: "C", module: 21, column: 1, line: 396 });
    expect(missing.some((i) => i.code === "MISSING_NATIVE_FIELD" && i.key === "area_no")).toBe(true);

    const badRange = validateNativeFields(schema, { ...nativeValues, area_no: 99 });
    expect(badRange.some((i) => i.key === "area_no")).toBe(true);

    const badEnum = validateNativeFields(schema, { ...nativeValues, area: "Z" });
    expect(badEnum.some((i) => i.key === "area")).toBe(true);
  });

  it("validateNativePoint_rejects_derived_key_drift_and_invalid_user_key", () => {
    const base = {
      kind: "nck_native",
      point_key: "native.C.1.21.1.396",
      parameters: nativeValues,
    };
    expect(validateNativePoint(base)).toEqual({ ok: true });
    const drift = validateNativePoint({ ...base, point_key: "native.C.1.21.1.397" });
    expect(drift.ok).toBe(false);
    if (!drift.ok) expect(drift.code).toBe("NATIVE_KEY_MISMATCH");

    const user = {
      ...base,
      point_key: "channel.program.status@channel=i:1",
      parameters: { ...nativeValues, identity_mode: "user" },
    };
    const badUser = validateNativePoint(user);
    expect(badUser.ok).toBe(false);
    if (!badUser.ok) expect(badUser.code).toBe("NATIVE_KEY_INVALID");
  });
});
