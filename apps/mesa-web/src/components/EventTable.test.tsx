// PR8 Gate（组件）：历史首屏渲染；occurred 缺失不冒充.
import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { EventTable } from "./EventTable";
import { makeEvent } from "../test/fixtures";

describe("EventTable", () => {
  it("历史首屏渲染固定列", () => {
    render(<EventTable events={[makeEvent(2), makeEvent(1)]} loading={false} onSelect={() => {}} />);
    expect(screen.getByText("alarm.condition")).toBeTruthy();
    expect(screen.getByText("overtemp")).toBeTruthy();
    expect(screen.getByText("raised")).toBeTruthy();
  });

  it("occurred_at 缺失显示 em dash", () => {
    render(
      <EventTable
        events={[makeEvent(1, { event: { occurred_at_ns: null } })]}
        loading={false}
        onSelect={() => {}}
      />,
    );
    // 发生时间列为 —（接收时间列仍有值，只断言至少一个 — 存在且行可点）
    expect(screen.getAllByText("—").length).toBeGreaterThan(0);
  });
});
