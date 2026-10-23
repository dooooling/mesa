// PR-D1 质量码分类（T1 / T2 / T3）。
//
// 语义边界（P0-3）：`quality_code` 是**异构十进制字符串**，同一个数字在不同
// driver 下含义不同（`3` 在 sinumerik-nck 是 S7 参考码表类别，在 focas2 是
// "参数类"）。因此分类的最小键是 `(driver_id, code)` 二元组：
// - 只有仓库内有显式定义的码才给一句话解释；
// - 其余一律 `UNKNOWN`（不解释），绝不猜厂商码表；
// - `DEVICE_ERROR` / `COMMUNICATION_LOST` 是 Core 写的**符号标记**，不是设备
//   返回码，任何情况下都不得解释成设备语义。
import { describe, expect, it } from "vitest";
import { resolveEndpointContexts } from "./deviceModel";
import {
  MISSING_CODE_TEXT,
  classifyCode,
  codeGroupTitle,
  codeHex,
  codeText,
  namespaceLabel,
  samplingBadge,
  shouldShowCodeSection,
} from "./qualityCode";

describe("qualityCode · 质量码分类（PR-D1）", () => {
  it("classifyCode_device_codes_only_for_known_drivers", () => {
    // 已知 driver 的数字码才进入"设备/协议返回码"命名空间。
    expect(classifyCode("sinumerik-nck", "5")).toBe("DEVICE_S7_ITEM");
    expect(classifyCode("opcua", "-2143289344")).toBe("PROTOCOL_STATUS");
    expect(classifyCode("focas2", "2")).toBe("MESA_CLASS");
    // 无 driver 信息 ⇒ 无法判定命名空间，保守降级为 UNKNOWN（不解释）。
    expect(classifyCode("", "5")).toBe("UNKNOWN");
  });

  it("classifyCode_never_explains_unknown_numbers", () => {
    // 0x0A 只在测试夹具出现、0x07 无任何仓库定义 ⇒ 只显示原值，不给文案。
    expect(codeText("sinumerik-nck", "10")).toBeNull();
    expect(codeText("sinumerik-nck", "7")).toBeNull();
    // 分类本身仍归属 S7 item 命名空间（设备返回的字节），只是不解释。
    expect(classifyCode("sinumerik-nck", "10")).toBe("DEVICE_S7_ITEM");
  });

  it("mesa_markers_are_never_device_codes", () => {
    for (const marker of ["DEVICE_ERROR", "COMMUNICATION_LOST"]) {
      expect(classifyCode("sinumerik-nck", marker)).toBe("MESA_MARKER");
      expect(classifyCode("opcua", marker)).toBe("MESA_MARKER");
      // 符号标记的解释里不得出现"设备"字样（它不是设备返回的码）。
      const text = codeText("sinumerik-nck", marker) ?? "";
      expect(text.length).toBeGreaterThan(0);
      expect(text).not.toContain("设备");
    }
  });

  it("focas_mesa_class_is_not_device_return_group", () => {
    // 缺陷 2：focas2 的 1..5 是 Mesa 自己的单点错误分类码（§4.2），
    // 不是厂家/设备返回码，不得与 S7/NCK 的设备返回码同组。
    expect(codeGroupTitle("MESA_CLASS")).toBe("Mesa 分类（非厂家码）");
    expect(codeGroupTitle("MESA_CLASS")).not.toBe(codeGroupTitle("DEVICE_S7_ITEM"));
    expect(codeGroupTitle("MESA_CLASS")).not.toBe(codeGroupTitle("PROTOCOL_STATUS"));
    expect(codeGroupTitle("DEVICE_S7_ITEM")).toBe("设备/协议返回码");
    expect(codeGroupTitle("PROTOCOL_STATUS")).toBe("设备/协议返回码");
  });

  it("missing_code_is_visible_for_non_good_rows_only", () => {
    // 裁决 1（U2 vs §4.2）：GOOD 无码隐藏（T7 依赖）；BAD 无码必须显示异常形态。
    expect(shouldShowCodeSection("GOOD", undefined)).toBe(false);
    expect(shouldShowCodeSection("good", undefined)).toBe(false);
    expect(shouldShowCodeSection("BAD", undefined)).toBe(true);
    expect(shouldShowCodeSection("UNCERTAIN", undefined)).toBe(true);
    // 有码的行任何质量都显示（含 GOOD，例如断线残留/异常数据）。
    expect(shouldShowCodeSection("GOOD", "5")).toBe(true);
    expect(MISSING_CODE_TEXT).toBe("未提供（异常形态，请报告）");
  });
});

// ---------------------------------------------------------------------------
// REST 响应 → 前端分类与展示（一期必需）
//
// 不 mock 分类逻辑：输入是 `/api/v1/points/latest` 与 `/api/v1/endpoints`
// 的**线上 JSON 文本**（`LatestEntry` 逐字序列化形态），`JSON.parse` 后走 UI
// 同一条路径（端点清单 → driver_id → 分类/文案/徽标）。目的：锁住"后端输出
// 什么形状，前端就解释成什么"，而不是只测纯函数。
//
// 关键线上事实（`crates/driver-manager/src/snapshot.rs:113-127`）：
// - `quality_code` 是十进制**字符串**或符号标记；
// - GOOD 行**没有该键**（`skip_serializing_if`），不是 null/空串。
// ---------------------------------------------------------------------------
const LATEST_RESPONSE = `{"points":[
  {"endpoint_id":"ep-nck","point_id":10,"point_key":"qc.native","key":"qc.native","type":"f64","value":0.0,"quality":"BAD","quality_code":"5","timestamp_ns":1700000000000000000,"value_origin":"CURRENT"},
  {"endpoint_id":"ep-nck","point_id":11,"point_key":"qc.good","key":"qc.good","type":"f64","value":1.0,"quality":"GOOD","timestamp_ns":1700000000000000000,"value_origin":"CURRENT"},
  {"endpoint_id":"ep-nck","point_id":12,"point_key":"qc.lost","key":"qc.lost","type":"f64","value":0.0,"quality":"BAD","quality_code":"COMMUNICATION_LOST","timestamp_ns":1700000000000000000,"value_origin":"LAST_KNOWN"}
],"count":3}`;

const ENDPOINTS_RESPONSE = `{"endpoints":[{"id":"ep-nck","name":"NCK","device_id":"d1","driver_id":"sinumerik-nck"}]}`;

interface RestRow {
  endpoint_id: string;
  point_id: number;
  quality: string;
  quality_code?: string;
  value_origin?: string;
}

describe("qualityCode · REST 响应驱动的前端分类与展示（一期必需）", () => {
  const latest = JSON.parse(LATEST_RESPONSE) as { points: RestRow[] };
  const endpoints = JSON.parse(ENDPOINTS_RESPONSE) as {
    endpoints: Array<{ id: string; name: string; device_id: string; driver_id: string }>;
  };
  // 与 useLivePointsSource 同路径：端点清单 → 上下文 → driverId。
  const ctx = resolveEndpointContexts(endpoints.endpoints, []);
  const driverIdOf = (row: RestRow) => ctx.get(row.endpoint_id)?.driverId ?? "";

  it("native_code_row_keeps_decimal_string_and_is_classified_as_device_code", () => {
    const row = latest.points[0];
    // 线上必须是字符串：JSON.parse 后仍是 string（不是 number）。
    // 后端若哪天改成数字，这条会先失败，而不是让分类悄悄降级。
    expect(typeof row.quality_code).toBe("string");
    expect(driverIdOf(row)).toBe("sinumerik-nck");
    expect(classifyCode(driverIdOf(row), row.quality_code, row.quality)).toBe("DEVICE_S7_ITEM");
    expect(namespaceLabel("DEVICE_S7_ITEM")).toBe("S7 item");
    expect(codeText(driverIdOf(row), row.quality_code)).toContain("地址错误类");
    expect(shouldShowCodeSection(row.quality, row.quality_code)).toBe(true);
    // 十六进制只是视图，线上格式仍是十进制字符串。
    expect(codeHex(row.quality_code)).toBe("0x05 (5)");
  });

  it("good_row_has_no_key_so_no_code_section_is_rendered", () => {
    const row = latest.points[1];
    // "键不存在"而不是 null：这是 GOOD 行的线上契约。
    expect("quality_code" in row).toBe(false);
    expect(classifyCode(driverIdOf(row), row.quality_code, row.quality)).toBe("NONE");
    expect(shouldShowCodeSection(row.quality, row.quality_code)).toBe(false);
    expect(codeText(driverIdOf(row), row.quality_code)).toBeNull();
  });

  it("communication_lost_marker_is_not_rendered_as_device_code", () => {
    const row = latest.points[2];
    expect(classifyCode(driverIdOf(row), row.quality_code, row.quality)).toBe("MESA_MARKER");
    expect(codeGroupTitle("MESA_MARKER")).toContain("不是设备返回码");
    // Core 本地标记的解释里不得出现"设备"字样。
    expect(codeText(driverIdOf(row), row.quality_code)).not.toContain("设备");
    // 断线覆盖：旧值 + 不可恢复提示（§5.3 徽标）。
    const badge = samplingBadge(row.value_origin, row.quality, row.quality_code);
    expect(badge.unrecoverable).toBe(true);
    expect(badge.notCurrent).toBe(true);
    expect(badge.text).toContain("断线前");
  });

  it("unknown_driver_code_is_shown_verbatim_without_explanation", () => {
    // 语义空间由 driver_id 决定：没有驱动信息就不解释。
    expect(classifyCode("", "5", "BAD")).toBe("UNKNOWN");
    expect(codeText("", "5")).toBeNull();
    expect(namespaceLabel("UNKNOWN")).toBe("未知命名空间");
    // 有码但未知语义：仍必须展示原值（不得静默隐藏）。
    expect(shouldShowCodeSection("BAD", "5")).toBe(true);
  });
});
