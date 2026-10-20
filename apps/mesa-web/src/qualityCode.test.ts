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
import { classifyCode, codeText } from "./qualityCode";

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
});
