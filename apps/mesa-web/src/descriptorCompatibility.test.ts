// A3：字段类型白名单与「未知类型」判定（纯函数门禁）。
//
// 重点锁三件事：
// 1. 已知文本类类型仍共用文本框（白名单不能被写成“只认 string”）；
// 2. 未知类型必须被识别出来（不得静默降级为可编辑文本）；
// 3. 判定扫**完整 Schema**：高级区/visible_if 隐藏字段都不能掩盖不支持。
import { describe, expect, it } from "vitest";
import type { FieldDescriptor, SchemaDescriptor } from "./types";
import {
  TEXT_FIELD_TYPES,
  isSupportedFieldType,
  isTextLikeFieldType,
  unsupportedFieldDetails,
  unsupportedFieldKeys,
  unsupportedFields,
  unsupportedFieldsInResources,
  unsupportedFieldsInValue,
} from "./descriptorCompatibility";

/** 构造字段；field_type 故意用 `as` 注入 wire 上可能出现的未知类型。 */
function field(
  key: string,
  fieldType: string,
  ui: FieldDescriptor["ui"] = {},
): FieldDescriptor {
  return {
    key,
    label: key.toUpperCase(),
    field_type: fieldType as FieldDescriptor["field_type"],
    required: false,
    validation: {},
    ui,
  };
}

const schema = (fields: FieldDescriptor[]): SchemaDescriptor => ({ fields });

describe("字段类型白名单", () => {
  it("已知文本类类型共用文本框（string/host/url/file/certificate_ref）", () => {
    for (const type of TEXT_FIELD_TYPES) {
      expect(isTextLikeFieldType(type)).toBe(true);
      expect(isSupportedFieldType(type)).toBe(true);
    }
  });

  it("专用控件类型也算已知（不会被误判成未知）", () => {
    for (const type of ["boolean", "enum", "secret", "integer", "number", "port", "duration"]) {
      expect(isTextLikeFieldType(type)).toBe(false);
      expect(isSupportedFieldType(type)).toBe(true);
    }
  });

  it("未知/缺失/非字符串类型一律不支持", () => {
    expect(isSupportedFieldType("geometry")).toBe(false);
    expect(isSupportedFieldType("")).toBe(false);
    expect(isSupportedFieldType(undefined)).toBe(false);
    expect(isSupportedFieldType(7)).toBe(false);
  });
});

describe("unsupportedFields（完整 Schema）", () => {
  it("返回未知类型的 key/label/原始类型名", () => {
    const out = unsupportedFields(schema([field("host", "host"), field("shape", "geometry")]));
    expect(out).toEqual([{ key: "shape", label: "SHAPE", fieldType: "geometry", advanced: false }]);
    expect(unsupportedFieldKeys(out)).toEqual(new Set(["shape"]));
  });

  it("高级区字段同样被检出（折叠不能掩盖不支持）", () => {
    const out = unsupportedFields(
      schema([field("host", "host"), field("shape", "geometry", { advanced: true })]),
    );
    expect(out.map((f) => f.key)).toEqual(["shape"]);
    expect(out[0]!.advanced).toBe(true);
    expect(unsupportedFieldDetails(out)).toEqual(["shape · geometry（高级）"]);
  });

  it("被 visible_if 隐藏的字段也在扫描范围内（不等用户切值才发现）", () => {
    const out = unsupportedFields(
      schema([
        field("host", "host"),
        field("shape", "geometry", { visible_if: { field: "mode", op: "eq", value: "x" } }),
      ]),
    );
    expect(out.map((f) => f.key)).toEqual(["shape"]);
  });

  it("空/缺失 Schema 不产生误报", () => {
    expect(unsupportedFields({ fields: [] })).toEqual([]);
    expect(unsupportedFields(null)).toEqual([]);
  });
});

describe("unsupportedFieldsInValue（限制影响范围）", () => {
  const s = schema([field("host", "host"), field("shape", "geometry")]);

  it("只报告配置里真的带着的未知字段", () => {
    expect(unsupportedFieldsInValue(s, { host: "1.2.3.4" })).toEqual([]);
    const out = unsupportedFieldsInValue(s, { host: "1.2.3.4", shape: { x: 1 } });
    expect(out.map((f) => f.key)).toEqual(["shape"]);
  });

  it("key 存在即算携带（值为 null 也不放过）", () => {
    expect(unsupportedFieldsInValue(s, { shape: null }).map((f) => f.key)).toEqual(["shape"]);
  });

  it("没有配置值时不做判断", () => {
    expect(unsupportedFieldsInValue(s, undefined)).toEqual([]);
    expect(unsupportedFieldsInValue(s, null)).toEqual([]);
  });
});

describe("unsupportedFieldsInResources（最终保存守卫：扫完整 Schema）", () => {
  const s = schema([field("host", "host"), field("shape", "geometry", { advanced: true })]);

  it("配置未携带未知必填字段也必须检出（驱动新增字段的唯一缺口）", () => {
    const out = unsupportedFieldsInResources([
      { resourceId: "probe", schema: s, values: [{ host: "1.2.3.4" }] },
    ]);
    expect(out.map((f) => f.key)).toEqual(["shape"]);
    // 未携带：存量配置根本没有这个键，旧守卫（unsupportedFieldsInValue）会返回空。
    expect(out[0]!.carried).toBe(false);
    expect(out[0]!.resourceId).toBe("probe");
    expect(out[0]!.advanced).toBe(true);
  });

  it("存量配置已携带时同样检出，并标注 carried", () => {
    const out = unsupportedFieldsInResources([
      { resourceId: "probe", schema: s, values: [{ host: "h", shape: { x: 1 } }] },
    ]);
    expect(out.map((f) => f.key)).toEqual(["shape"]);
    expect(out[0]!.carried).toBe(true);
  });

  it("同一资源多条提交值：任一条携带即算携带", () => {
    const out = unsupportedFieldsInResources([
      { resourceId: "probe", schema: s, values: [{ host: "a" }, { shape: null }] },
    ]);
    expect(out[0]!.carried).toBe(true);
  });

  it("只扫传入的提交资源（未使用资源不在入参里 ⇒ 不会误报）", () => {
    expect(unsupportedFieldsInResources([])).toEqual([]);
  });

  it("多个提交资源各自独立标注", () => {
    const clean = schema([field("host", "host")]);
    const out = unsupportedFieldsInResources([
      { resourceId: "a", schema: clean, values: [{}] },
      { resourceId: "b", schema: s, values: [{}] },
    ]);
    expect(out.map((f) => `${f.resourceId}:${f.key}:${f.carried}`)).toEqual([
      "b:shape:false",
    ]);
  });
});
