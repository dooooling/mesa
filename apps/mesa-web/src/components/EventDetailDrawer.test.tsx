// PR8 Gate（组件）：详情抽屉渲染 condition / attributes / 三层时间；无 Acknowledge 按钮。
import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { EventDetailDrawer } from "./EventDetailDrawer";
import { makeEvent } from "../test/fixtures";

describe("EventDetailDrawer", () => {
  it("渲染 condition、attributes 与三个时间", () => {
    const ev = makeEvent(9, { event: { attributes: { axis: 1, phase: "run" } } });
    render(<EventDetailDrawer event={ev} onClose={() => {}} />);
    expect(screen.getByText("SIM-ALARM-100")).toBeTruthy();
    expect(screen.getByText("phase")).toBeTruthy();
    expect(screen.getByText("occurred_at")).toBeTruthy();
    expect(screen.getByText("published_at")).toBeTruthy();
    expect(screen.getByText("received_at")).toBeTruthy();
  });

  it("绝无确认/控制类按钮", () => {
    const ev = makeEvent(9);
    const { container } = render(<EventDetailDrawer event={ev} onClose={() => {}} />);
    const text = container.textContent ?? "";
    expect(text).not.toContain("Acknowledge");
    expect(text).not.toContain("确认报警");
  });
});
