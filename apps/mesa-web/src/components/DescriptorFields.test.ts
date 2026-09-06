// PR8 Gate（P1-4）：schema defaults 物化——UI 显示即保存值。
import { describe, expect, it } from "vitest";
import { materializeSchemaDefaults } from "./DescriptorFields";
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
