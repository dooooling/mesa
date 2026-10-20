// PR-D1 点详情抽屉（T6 / T6b / T7 / T8）。
//
// 语义边界（P0-3）：抽屉必须把"值的来源"（`value_origin`）、"质量"
// （`quality`）、"原因码"（`(driver_id, quality_code)`）三者分开显示：
// - `LAST_KNOWN` 必带"未更新"，`PLACEHOLDER` 必带"无有效值"，不得渲染成当前值；
// - `CURRENT + BAD` 只表示"本次尝试返回了这个失败结果"，**禁用绿色**（P1-1）；
// - Mesa 符号标记不是设备返回码；nck 的 3/4/5 只给"仓库现有 S7 参考码表"类别，
//   **不得**出现具体根因（§4.4 禁用清单）。
import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { PointDetailDrawer, type PointIdentity } from "./PointDetailDrawer";
import { MISSING_CODE_TEXT } from "../qualityCode";
import type { DevicePointView } from "../workspace/useDeviceWorkspaceData";

const T0 = 1_700_000_000_000;

function view(over: Partial<DevicePointView> = {}): DevicePointView {
  return {
    endpoint_id: "lx-nck",
    key: "native.A.1.26.30110.1",
    point_key: "native.A.1.26.30110.1",
    point_id: 445,
    quality: "BAD",
    type: "u32",
    value: 0,
    timestamp_ns: (T0 - 500) * 1e6,
    value_origin: "LAST_KNOWN",
    quality_code: "COMMUNICATION_LOST",
    driverId: "sinumerik-nck",
    displayKey: "native.A.1.26.30110.1",
    sourceText: null,
    ageMs: 500,
    derived: "BAD",
    endpointName: "NCK",
    deviceId: "cnc-01",
    deviceName: "CNC-01",
    ...over,
  };
}

interface DrawerOpts {
  /** 当前实时集合（默认只含 point 本身）。 */
  points?: DevicePointView[];
  /** 打开时的定位标识；显式 `null` = 抽屉关闭。 */
  selected?: PointIdentity | null;
}

function drawerEl(point: DevicePointView, opts: DrawerOpts = {}) {
  return (
    <MemoryRouter>
      <PointDetailDrawer
        deviceId="cnc-01"
        deviceName="CNC-01"
        points={opts.points ?? [point]}
        selected={
          opts.selected === undefined
            ? { endpoint_id: point.endpoint_id, point_id: point.point_id }
            : opts.selected
        }
        onClose={() => {}}
      />
    </MemoryRouter>
  );
}

function renderDrawer(point: DevicePointView, opts: DrawerOpts = {}) {
  return render(drawerEl(point, opts));
}

describe("PointDetailDrawer · 采样与原因码（PR-D1）", () => {
  it("drawer_marks_last_known_as_not_current", () => {
    renderDrawer(view());
    // 旧值必须带"未更新"，且断线覆盖不可反推原始项级码。
    expect(screen.getByText(/旧值，未更新（断线前）/)).toBeTruthy();
    expect(screen.getByText(/此前的项级码已被覆盖，不可恢复/)).toBeTruthy();
    // 码区正文不得把该标记说成"设备返回"（它是 Core 本地标记）。
    expect(screen.getByTestId("quality-code-body").textContent).not.toContain("设备返回");
    // 不得把 LAST_KNOWN 渲染成当前值。
    expect(screen.queryByText(/当前有效值/)).toBeNull();
  });

  it("drawer_current_bad_is_not_success", () => {
    // CURRENT + BAD + 数字码：本次尝试返回失败结果，但绝不是"采样成功"。
    renderDrawer(view({ value_origin: "CURRENT", quality: "BAD", quality_code: "5" }));
    const badge = screen.getByText("本次返回失败");
    const tag = badge.closest(".ant-tag");
    expect(tag).toBeTruthy();
    expect(tag!.className).toMatch(/red/);
    expect(tag!.className).not.toMatch(/green/);
    expect(screen.queryByText(/当前有效值/)).toBeNull();
    expect(screen.queryByText(/本次采样/)).toBeNull();
  });

  it("drawer_hides_code_section_for_good_rows", () => {
    // T7 按裁决收窄：只断言 **GOOD** 无码隐藏；BAD 无码必须可见
    //（见 drawer_shows_missing_code_for_bad_rows），不得再用本用例覆盖 BAD。
    // GOOD 且无码 ⇒ 整块不渲染（GOOD 行零额外 DOM，§8.3）。
    renderDrawer(
      view({
        quality: "GOOD",
        derived: "GOOD",
        value_origin: "CURRENT",
        quality_code: undefined,
      }),
    );
    expect(screen.queryByText("原因码")).toBeNull();
    expect(screen.getByText("当前有效值")).toBeTruthy();
  });

  it("drawer_shows_missing_code_for_bad_rows", () => {
    // 裁决 1（U2 vs §4.2）：BAD 无码是异常形态，必须可见并明说"请报告"，
    // 不得按 U2 的"无码即隐藏"处理（那会让操作员以为没有原因可看）。
    renderDrawer(
      view({
        quality: "BAD",
        derived: "BAD",
        value_origin: "CURRENT",
        quality_code: undefined,
      }),
    );
    expect(screen.getByText("原因码")).toBeTruthy();
    expect(screen.getByText(MISSING_CODE_TEXT)).toBeTruthy();
    // 没有码就没有命名空间/解释，更不得把缺失说成设备返回码。
    const body = screen.getByTestId("quality-code-body").textContent ?? "";
    expect(body).not.toContain("不解释");
    expect(body).not.toContain("设备返回");
    expect(screen.queryByText("S7 item")).toBeNull();
  });

  it("drawer_follows_live_updates_while_open", () => {
    // 缺陷 1（P1）：抽屉只持有定位标识，渲染时从**当前实时集合**查找。
    // 打开期间收到 SSE 更新必须随之变化；点消失必须明确表现，不得显示旧值。
    const open = view({
      quality: "BAD",
      derived: "BAD",
      value_origin: "CURRENT",
      quality_code: "5",
    });
    const selected: PointIdentity = {
      endpoint_id: open.endpoint_id,
      point_id: open.point_id,
    };
    const { rerender } = render(drawerEl(open, { points: [open], selected }));
    expect(screen.getByText("0x05 (5)")).toBeTruthy();

    // SSE 断线覆盖：同一 point_id，码 5 → COMMUNICATION_LOST。
    const lost = view({
      quality: "BAD",
      derived: "BAD",
      value_origin: "LAST_KNOWN",
      quality_code: "COMMUNICATION_LOST",
    });
    rerender(drawerEl(open, { points: [lost], selected }));
    expect(screen.queryByText("0x05 (5)")).toBeNull();
    expect(screen.getByText(/此前的项级码已被覆盖，不可恢复/)).toBeTruthy();
    expect(screen.getByText(/旧值，未更新（断线前）/)).toBeTruthy();

    // SSE snapshot 全量替换后该点消失：不得继续展示旧快照。
    rerender(drawerEl(open, { points: [], selected }));
    expect(screen.getByText(/已从实时快照中消失/)).toBeTruthy();
    expect(screen.queryByText(/旧值，未更新（断线前）/)).toBeNull();
    expect(screen.queryByText(/此前的项级码已被覆盖，不可恢复/)).toBeNull();
  });

  it("drawer_shows_reference_class_not_root_cause", () => {
    const first = renderDrawer(
      view({ quality_code: "5", value_origin: "CURRENT", quality: "BAD" }),
    );
    // 原始码保留 + 只给"仓库现有 S7 参考码表"类别（首版收窄口径 P1-3）。
    expect(screen.getByText("0x05 (5)")).toBeTruthy();
    expect(
      screen.getByText("仓库现有 S7 参考码表：地址错误类（可能关联，需结合协议层确认）"),
    ).toBeTruthy();
    const body = screen.getByTestId("quality-code-body").textContent ?? "";
    // §4.4 禁用清单：不得出现具体根因。
    expect(body).not.toContain("DB 不存在");
    expect(body).not.toContain("CPU 保护等级");
    expect(body).not.toContain("PUT/GET");
    first.unmount();

    // 0x0A：仓库无码表 ⇒ 只显示原值，无任何类别文案。
    renderDrawer(view({ quality_code: "10", value_origin: "CURRENT", quality: "BAD" }));
    expect(screen.getByText("0x0A (10)")).toBeTruthy();
    const unknownBody = screen.getByTestId("quality-code-body").textContent ?? "";
    expect(unknownBody).not.toContain("仓库现有 S7 参考码表");
    expect(unknownBody).not.toContain("设备");
  });
});
