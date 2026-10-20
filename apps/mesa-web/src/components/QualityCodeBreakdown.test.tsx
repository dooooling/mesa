// PR-D1 质量码分解（T9）。
//
// 断言点（§10.2 T9）：分区标题存在；同一数字在不同 driver 下**分开计数**
// （`3` 在 sinumerik-nck 是 S7 参考码表类别、在 focas2 是"参数类"，相加即错）。
// 统计只基于当前快照，Popover 底部必须带固定限制语。
import { describe, expect, it } from "vitest";
import { render, screen, within } from "@testing-library/react";
import {
  BREAKDOWN_LIMITATION,
  QualityCodeBreakdownContent,
  type BreakdownPoint,
} from "./QualityCodeBreakdown";

const p = (over: Partial<BreakdownPoint>): BreakdownPoint => ({
  endpoint_id: "ep-a",
  driverId: "sinumerik-nck",
  quality: "BAD",
  ...over,
});

describe("QualityCodeBreakdown · 质量码分解（PR-D1）", () => {
  it("breakdown_groups_by_origin_and_endpoint", () => {
    const points: BreakdownPoint[] = [
      // 同一端点、同一 driver 的同一数字码 ×2
      p({ quality_code: "5" }),
      p({ quality_code: "5" }),
      // 同一数字码、不同 driver（focas2 的 5 = Mesa 连接类）⇒ 必须单独一行
      p({ endpoint_id: "ep-b", driverId: "focas2", quality_code: "5" }),
      // Mesa 标记（不是设备返回码）
      p({ quality_code: "COMMUNICATION_LOST" }),
      // nck 的 0x0A：属 S7 item 命名空间但仓库无定义（只显示原值）
      p({ quality_code: "10" }),
      // 无 driver 信息：命名空间不可判定 ⇒ 未知命名空间（不解释）
      p({ endpoint_id: "ep-c", driverId: "", quality_code: "5" }),
    ];

    render(<QualityCodeBreakdownContent points={points} />);

    // 分区标题存在（Mesa 标记与设备/协议返回码分开）。
    expect(screen.getByText("Mesa 标记（不是设备返回码）")).toBeTruthy();
    expect(screen.getByText("设备/协议返回码")).toBeTruthy();
    expect(screen.getByText("未知命名空间（不解释）")).toBeTruthy();

    // 同一数字（5）在不同 driver 下是两行，各自计数，绝不合并成 3。
    const fiveRows = screen
      .getAllByTestId("code-row")
      .filter((row) => row.textContent?.includes("0x05 (5)"));
    // 三行：ep-a/nck、ep-b/focas2、ep-c/未知驱动。
    expect(fiveRows).toHaveLength(3);

    const nckRow = fiveRows.find((row) => row.textContent?.includes("sinumerik-nck"));
    const focasRow = fiveRows.find((row) => row.textContent?.includes("focas2"));
    const unknownDriverRow = fiveRows.find((row) => row.textContent?.includes("未知驱动"));
    expect(nckRow).toBeTruthy();
    expect(focasRow).toBeTruthy();
    expect(unknownDriverRow).toBeTruthy();
    expect(within(nckRow!).getByTestId("code-count").textContent).toBe("2");
    expect(within(focasRow!).getByTestId("code-count").textContent).toBe("1");
    expect(within(unknownDriverRow!).getByTestId("code-count").textContent).toBe("1");
    expect(unknownDriverRow!.textContent).toContain("未知命名空间");

    // 端点身份参与分组（同 driver 不同端点也分开）。
    expect(nckRow!.textContent).toContain("ep-a");
    expect(focasRow!.textContent).toContain("ep-b");

    // 0x0A 属 S7 item 命名空间但仓库无定义：只显示原值 + 命名空间，无类别文案。
    const undefinedRow = screen
      .getAllByTestId("code-row")
      .find((row) => row.textContent?.includes("0x0A (10)"));
    expect(undefinedRow).toBeTruthy();
    expect(undefinedRow!.textContent).toContain("S7 item");
    expect(undefinedRow!.textContent).not.toContain("仓库现有 S7 参考码表");

    // 限制语与导出入口（同一 Popover 内）。
    expect(screen.getByText(BREAKDOWN_LIMITATION)).toBeTruthy();
    expect(screen.getByText("导出原始快照")).toBeTruthy();
  });

  it("focas_mesa_class_has_its_own_partition", () => {
    // 缺陷 2：focas2 的 1..5 是 Mesa 自己的分类码（非厂家码），
    // 必须单独分组，不得与 S7/NCK 的设备返回码同组展示。
    const points: BreakdownPoint[] = [
      p({ endpoint_id: "ep-nck", driverId: "sinumerik-nck", quality_code: "5" }),
      p({ endpoint_id: "ep-focas", driverId: "focas2", quality_code: "5" }),
    ];
    render(<QualityCodeBreakdownContent points={points} />);

    const partitions = screen.getAllByTestId("code-partition");
    const devicePartition = partitions.find((el) =>
      el.textContent?.includes("设备/协议返回码"),
    );
    const mesaPartition = partitions.find((el) =>
      el.textContent?.includes("Mesa 分类（非厂家码）"),
    );
    expect(devicePartition).toBeTruthy();
    expect(mesaPartition).toBeTruthy();
    // focas2 的码只出现在 Mesa 分类分区，绝不混进设备/协议返回码分区。
    expect(within(devicePartition!).queryByText("focas2")).toBeNull();
    expect(within(mesaPartition!).getByText("focas2")).toBeTruthy();
    expect(within(mesaPartition!).getByText("0x05 (5)")).toBeTruthy();
    // nck 的 5 仍在设备/协议分区。
    expect(within(devicePartition!).getByText("sinumerik-nck")).toBeTruthy();
  });
});
