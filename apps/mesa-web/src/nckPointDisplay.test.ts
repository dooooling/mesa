// P1-2：点位行两层信息（易读名称 / 地址 / 定义版本）的纯函数门禁。
//
// 重点锁两条硬约束：
// 1. 原生地址没有可靠业务名称 ⇒ 只能显示「原生点位」，不得按参数编造业务名；
// 2. 名称只能来自定义目录的 label（或 variable_id），不得由地址推测。
import { describe, expect, it } from "vitest";
import { nckPointDisplay, type NckDisplayPoint } from "./nckPointDisplay";
import type { NckDefinitionLike } from "./nckPointModel";

function semanticDefinition(overrides: Partial<NckDefinitionLike> = {}): NckDefinitionLike {
  return {
    variable_id: "syn.axis.position",
    revision: 1,
    family: "828d",
    scope: "collect",
    label: { zh: "轴位置", en: "Axis position" },
    targets: ["channel", "axis"],
    target_required: ["channel", "axis"],
    arguments: [{ key: "index", label: { zh: "R 参数号" }, value_type: "integer" }],
    ...overrides,
  };
}

function semanticPoint(overrides: Partial<NckDisplayPoint> = {}): NckDisplayPoint {
  return {
    kind: "nck_variable",
    point_key: "syn.axis.position@channel=1,axis=2",
    parameters: {
      variable_id: "syn.axis.position",
      definition_revision: 1,
      target: { channel: 1, axis: 2 },
      arguments: { index: 5 },
    },
    ...overrides,
  };
}

describe("原生点位：没有可靠业务名称就不编造", () => {
  const nativePoint: NckDisplayPoint = {
    kind: "nck_native",
    point_key: "native.A.1.26.30110.1",
    parameters: { area: "A", area_no: 1, module: 26, column: 30110, line: 1 },
  };

  it("uses_fixed_title_and_structured_address", () => {
    const display = nckPointDisplay(nativePoint, null);
    expect(display.title).toBe("原生点位");
    // 五段地址按 area / area_no / module / column / line 原样拼接。
    expect(display.address).toBe("区域 A · 区域号 1 · 模块 26 · 列 30110 · 行 1");
    expect(display.revision).toBe("定义版本：不适用（高级原生点位）");
  });

  it("never_invents_business_name_from_address", () => {
    const display = nckPointDisplay(nativePoint, null);
    // 不得出现「通道进给倍率」之类的业务猜测：标题恒为「原生点位」。
    expect(display.title).not.toMatch(/通道|进给|倍率|转速|位置/);
    expect(display.address).not.toMatch(/通道|进给|倍率/);
  });

  it("incomplete_native_fields_are_reported_without_fabricating", () => {
    const display = nckPointDisplay(
      { kind: "nck_native", point_key: "native.A.1", parameters: { area: "A", area_no: 1 } },
      null,
    );
    expect(display.title).toBe("原生点位");
    expect(display.address).toBe("区域 A · 区域号 1");
  });

  it("native_address_is_labeled_not_bare_numbers(self)", () => {
    // §2.6-4：原生地址必须是「字段标签 + 值」，不能是无说明的五段数字。
    const display = nckPointDisplay(nativePoint, null);
    expect(display.address).toBe("区域 A · 区域号 1 · 模块 26 · 列 30110 · 行 1");
  });

  it("native_translation_table_then_schema_label_then_key", () => {
    // 翻译表命中的字段用中文标签；未收录的字段不影响地址摘要，也不编造业务名。
    const display = nckPointDisplay(nativePoint, null, {
      nativeSchema: {
        fields: [
          { key: "module", label: "Module (EN)", field_type: "integer", required: false, validation: {}, ui: {} },
        ],
      },
    });
    expect(display.address).toBe("区域 A · 区域号 1 · 模块 26 · 列 30110 · 行 1");
    expect(display.address).not.toMatch(/进给|倍率|转速/);
  });
});

describe("语义点位：名称只取定义 label，地址由 Target/Arguments 拼出", () => {
  it("shows_definition_label_and_readable_address", () => {
    const display = nckPointDisplay(semanticPoint(), semanticDefinition());
    expect(display.title).toBe("轴位置");
    expect(display.address).toBe("通道 1 · 轴 2 · R 参数号 5");
    expect(display.revision).toBe("定义版本：rev 1");
  });

  it("falls_back_to_variable_id_when_label_missing", () => {
    const display = nckPointDisplay(
      semanticPoint(),
      semanticDefinition({ label: undefined }),
    );
    // label 缺失时退回变量 ID（仍是目录事实），而不是编一个业务名。
    expect(display.title).toBe("syn.axis.position");
  });

  it("drive_object_address_keeps_entry_identity", () => {
    const display = nckPointDisplay(
      {
        kind: "nck_variable",
        point_key: "syn.drive.param@drive_object=26,entry=nc_variable",
        parameters: {
          variable_id: "syn.drive.param",
          definition_revision: 3,
          target: { drive_object: { number: 26, entry: "nc_variable" } },
          arguments: {},
        },
      },
      {
        variable_id: "syn.drive.param",
        revision: 3,
        targets: ["drive_object"],
        target_required: ["drive_object"],
        arguments: [],
      },
    );
    expect(display.address).toBe("驱动对象 26（入口 nc_variable）");
    expect(display.revision).toBe("定义版本：rev 3");
  });

  it("missing_definition_still_renders_parameters_without_guessing", () => {
    const display = nckPointDisplay(
      {
        kind: "nck_variable",
        point_key: "legacy@channel=9",
        parameters: {
          variable_id: "legacy.variable",
          definition_revision: 1,
          target: { channel: 9 },
          arguments: {},
        },
      },
      null,
    );
    expect(display.title).toBe("legacy.variable");
    expect(display.address).toBe("通道 9");
    expect(display.revision).toBe("定义版本：rev 1");
  });
});

