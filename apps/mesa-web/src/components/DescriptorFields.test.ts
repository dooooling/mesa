// PR8 Gate（P1-4）：schema defaults 物化——UI 显示即保存值。
import { describe, expect, it } from "vitest";
import { materializeSchemaDefaults, parseTsapInput } from "./DescriptorFields";
import type { SchemaDescriptor } from "../types";

const schema = (fields: SchemaDescriptor["fields"]): SchemaDescriptor => ({ fields });

describe("materializeSchemaDefaults", () => {
  it("只拾取带 default 的字段", () => {
    const out = materializeSchemaDefaults(
      schema([
        { key: "limit", label: "Limit", field_type: "integer", required: true, default: 100, validation: {}, ui: {} },
        { key: "mode", label: "Mode", field_type: "string", required: false, validation: {}, ui: {} },
      ]),
    );
    expect(out).toEqual({ limit: 100 });
  });

  it("空 schema 得空对象", () => {
    expect(materializeSchemaDefaults(schema([]))).toEqual({});
  });
});

describe("parseTsapInput（NCK TSAP 双格式）", () => {
  it("十六进制与十进制等价", () => {
    expect(parseTsapInput("0x0100")).toBe(256);
    expect(parseTsapInput("0X0100")).toBe(256);
    expect(parseTsapInput("0x0102")).toBe(258);
    expect(parseTsapInput("0100h")).toBe(256);
    expect(parseTsapInput("0102H")).toBe(258);
    expect(parseTsapInput("  0x100  ")).toBe(256);
    expect(parseTsapInput("256")).toBe(256);
    expect(parseTsapInput("258")).toBe(258);
    expect(parseTsapInput("0")).toBe(0);
    expect(parseTsapInput("65535")).toBe(65535);
    expect(parseTsapInput("0xFFFF")).toBe(65535);
  });

  it("非法原样返回（前端标红，后端拒绝）", () => {
    expect(parseTsapInput("")).toBe("");
    expect(parseTsapInput("0x")).toBe("0x");
    expect(parseTsapInput("0x10000")).toBe("0x10000");
    expect(parseTsapInput("0xZZ")).toBe("0xZZ");
    expect(parseTsapInput("70000")).toBe("70000");
    expect(parseTsapInput("-1")).toBe("-1");
    expect(parseTsapInput("abc")).toBe("abc");
  });
});
