// A4：通用「字段标签 + 值」摘要（纯函数门禁）。
//
// 摘要只依据既有事实（Descriptor 字段 label / 字段 key / 值本身），
// 不新增 summary_template 契约，也不做表达式：测试锁定键序、回落与格式。
import { describe, expect, it } from "vitest";
import type { FieldDescriptor, SchemaDescriptor } from "./types";
import {
  formatSummaryValue,
  pointSummary,
  schemaLabelResolver,
  summarizeParameters,
} from "./pointSummary";

const field = (key: string, label: string): FieldDescriptor => ({
  key,
  label,
  field_type: "string",
  required: false,
  validation: {},
  ui: {},
});

const schema: SchemaDescriptor = {
  fields: [field("unit", "寄存器"), field("encoding", "编码")],
};

describe("summarizeParameters（标签 + 值）", () => {
  it("用 Schema 字段 label 拼摘要", () => {
    expect(
      summarizeParameters({ unit: 3, encoding: "BE" }, { labelOf: schemaLabelResolver(schema) }),
    ).toBe("寄存器 3 · 编码 BE");
  });

  it("按 Schema 声明顺序输出（对象键序漂移不影响展示）", () => {
    const reversed: SchemaDescriptor = {
      fields: [field("b", "B 字段"), field("a", "A 字段")],
    };
    expect(
      summarizeParameters(
        { a: 1, b: 2 },
        { labelOf: schemaLabelResolver(reversed), order: reversed.fields.map((f) => f.key) },
      ),
    ).toBe("B 字段 2 · A 字段 1");
  });

  it("Schema 未声明的键按参数自身顺序补在后面，标签退回 key", () => {
    expect(
      summarizeParameters(
        { unit: 3, extra: "x" },
        { labelOf: schemaLabelResolver(schema), order: schema.fields.map((f) => f.key) },
      ),
    ).toBe("寄存器 3 · extra x");
  });

  it("跳过未设置项（undefined / null / 空串）", () => {
    expect(summarizeParameters({ a: undefined, b: null, c: "", d: 0, e: false })).toBe(
      "d 0 · e false",
    );
  });

  it("嵌套对象展开一层，不整串 JSON", () => {
    expect(summarizeParameters({ target: { channel: 1, axis: 2 } })).toBe(
      "target（channel 1 / axis 2）",
    );
    expect(summarizeParameters({ target: { channel: 1 } })).not.toContain("{");
  });

  it("自定义 formatEntry 可覆盖个别键（渲染钩子，不是模板语言）", () => {
    expect(
      summarizeParameters(
        { drive_object: { number: 26, entry: "nc" }, channel: 1 },
        {
          formatEntry: (key, value) =>
            key === "drive_object"
              ? `驱动对象 ${(value as { number: number }).number}`
              : undefined,
        },
      ),
    ).toBe("驱动对象 26 · channel 1");
  });

  it("没有可展示项时返回空串（由调用方决定回落）", () => {
    expect(summarizeParameters({})).toBe("");
    expect(summarizeParameters(null)).toBe("");
  });
});

describe("pointSummary（无标签时回落 point_key）", () => {
  it("有参数时给出可读摘要", () => {
    expect(pointSummary({ point_key: "register.value", parameters: { unit: 3 }, schema })).toBe(
      "寄存器 3",
    );
  });

  it("参数为空时回落 point_key", () => {
    expect(pointSummary({ point_key: "register.value", parameters: {}, schema })).toBe(
      "register.value",
    );
  });

  it("没有 Schema 时用字段 key 而不是猜业务名", () => {
    expect(pointSummary({ point_key: "p", parameters: { unit: 3 } })).toBe("unit 3");
  });
});

describe("formatSummaryValue", () => {
  it("标量/数组/对象都可读，不产生 [object Object]", () => {
    expect(formatSummaryValue("A")).toBe("A");
    expect(formatSummaryValue(3)).toBe("3");
    expect(formatSummaryValue(false)).toBe("false");
    expect(formatSummaryValue([1, 2])).toBe("1, 2");
    expect(formatSummaryValue({ a: 1, b: "x" })).toBe("a=1 / b=x");
    expect(formatSummaryValue(null)).toBe("—");
  });
});
