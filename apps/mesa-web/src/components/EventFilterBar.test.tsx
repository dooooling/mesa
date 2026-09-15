// M3.5 回归：EventFilterBar（常用 + 高级 + debounce）。
// - Select/时间即时生效；文本型（Category/Kind/Code/Condition/Severity）
//   400ms debounce 后才 onChange（避免每键一次历史 reload）；
// - 外部 value 变化（重置）时本地输入同步清空，不残留。
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, render, screen, fireEvent } from "@testing-library/react";
import { EventFilterBar } from "./EventFilterBar";
import { EMPTY_EVENT_FILTER_FORM, type EventFilterForm } from "../events/filters";

function renderBar(value: EventFilterForm, onChange: (n: EventFilterForm) => void) {
  return render(<EventFilterBar value={value} onChange={onChange} onReset={() => onChange(EMPTY_EVENT_FILTER_FORM)} />);
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("M3.5 EventFilterBar", () => {
  it("时间输入即时生效（无 debounce）", () => {
    const onChange = vi.fn();
    renderBar(EMPTY_EVENT_FILTER_FORM, onChange);
    const from = screen.getByPlaceholderText("接收时间起");
    fireEvent.change(from, { target: { value: "2026-09-14T10:00" } });
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange.mock.calls[0][0]).toMatchObject({
      from_ns: new Date("2026-09-14T10:00").getTime() * 1e6,
    });
  });

  it("文本型 400ms debounce：快输只提交一次", async () => {
    const onChange = vi.fn();
    renderBar(EMPTY_EVENT_FILTER_FORM, onChange);
    // 展开高级
    fireEvent.click(screen.getByText(/高级筛选/));
    const input = screen.getByPlaceholderText("Category");
    fireEvent.change(input, { target: { value: "a" } });
    fireEvent.change(input, { target: { value: "al" } });
    fireEvent.change(input, { target: { value: "alarm" } });
    // debounce 窗内不提交
    await act(async () => {
      await vi.advanceTimersByTimeAsync(300);
    });
    expect(onChange).not.toHaveBeenCalled();
    // 窗后提交一次，且为最终值
    await act(async () => {
      await vi.advanceTimersByTimeAsync(200);
    });
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange.mock.calls[0][0]).toMatchObject({ category: "alarm" });
  });

  it("外部重置时本地输入同步清空", async () => {
    const onChange = vi.fn();
    const { rerender } = renderBar({ ...EMPTY_EVENT_FILTER_FORM, category: "alarm" }, onChange);
    fireEvent.click(screen.getByText(/高级筛选/));
    expect((screen.getByPlaceholderText("Category") as HTMLInputElement).value).toBe("alarm");
    // 外部回到 EMPTY（重置）：本地输入跟随清空
    rerender(
      <EventFilterBar value={EMPTY_EVENT_FILTER_FORM} onChange={onChange} onReset={() => {}} />,
    );
    expect((screen.getByPlaceholderText("Category") as HTMLInputElement).value).toBe("");
  });
});
