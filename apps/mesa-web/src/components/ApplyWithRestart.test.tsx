// P0-2：统一生命周期条的按钮/文案状态机。
//
// 四个状态各自钉死：已停止（保存配置 + 保存后保持停止）、运行中
// （停止并应用 + 应用后重新启动）、保存中（处理中… + 禁用）、完成后
// （明确告知配置已保存后的真实落点）。
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { ApplyWithRestart } from "./ApplyWithRestart";

function renderBar(overrides: Partial<Parameters<typeof ApplyWithRestart>[0]> = {}) {
  const onApply = vi.fn();
  render(
    <ApplyWithRestart
      running={false}
      applying={false}
      canApply
      applyLabel="保存配置"
      onApply={onApply}
      {...overrides}
    />,
  );
  return { onApply };
}

describe("P0-2：已停止", () => {
  it("shows_save_only_copy_and_never_starts", async () => {
    const user = userEvent.setup();
    const { onApply } = renderBar();
    const button = screen.getByTestId("apply-button");
    expect(button.textContent).toBe("保存配置");
    expect(screen.getByTestId("apply-stopped-note").textContent).toContain("保存后保持停止");
    await user.click(button);
    // 已停止时永远是 apply(false)：按钮文案不得承诺启动。
    expect(onApply).toHaveBeenCalledWith(false);
  });

  it("completion_after_stopped_save_says_currently_stopped", () => {
    renderBar({ result: "stopped" });
    expect(screen.getByTestId("apply-result").textContent).toContain("配置已保存，当前停止");
  });
});

describe("P0-2：运行中", () => {
  it("shows_stop_and_apply_with_restart_checkbox_default_on", async () => {
    const user = userEvent.setup();
    const { onApply } = renderBar({ running: true });
    const button = screen.getByTestId("apply-button");
    expect(button.textContent).toBe("停止并应用");
    expect(screen.getByText(/停止 → 写入配置 → 启动/)).toBeTruthy();
    const checkbox = screen.getByRole("checkbox") as HTMLInputElement;
    expect(checkbox.checked).toBe(true);
    await user.click(button);
    expect(onApply).toHaveBeenCalledWith(true);
  });

  it("unchecking_restart_keeps_endpoint_stopped_and_copy_matches", async () => {
    const user = userEvent.setup();
    const { onApply } = renderBar({ running: true });
    await user.click(screen.getByRole("checkbox"));
    // 取消勾选后说明必须改成“保持停止”，不能继续写启动。
    expect(screen.getByText(/停止 → 写入配置，保存后保持停止/)).toBeTruthy();
    await user.click(screen.getByTestId("apply-button"));
    expect(onApply).toHaveBeenCalledWith(false);
  });

  it("completion_after_restart_says_acquisition_started", () => {
    renderBar({ running: true, result: "restarted" });
    expect(screen.getByTestId("apply-result").textContent).toContain("配置已保存，采集已启动");
  });
});

describe("P0-2：保存中 / 完成失败", () => {
  it("applying_shows_processing_and_disables_button", () => {
    const { onApply } = renderBar({ applying: true });
    const button = screen.getByTestId("apply-button") as HTMLButtonElement;
    expect(button.textContent).toBe("处理中…");
    expect(button.disabled).toBe(true);
    // 防重复操作：即使强行触发 click 也不得再次 onApply。
    fireEvent.click(button);
    expect(onApply).not.toHaveBeenCalled();
  });

  it("restart_failure_is_reported_without_claiming_success", () => {
    renderBar({ running: true, result: "restart-failed" });
    const alert = screen.getByTestId("apply-result");
    expect(alert.textContent).toContain("配置已保存，但采集启动失败");
    expect(alert.textContent).not.toContain("采集已启动");
  });
});
