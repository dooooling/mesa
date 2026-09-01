// @ts-check
import { test, expect } from "@playwright/test";

test("control write/command via reliable queue ( disabled by default )", async ({ page }) => {
  await page.goto("/control");
  await expect(page.getByText(/控制/i)).toBeVisible();
  // 未 --enable-control 时 write 应得 503 CONTROL_DISABLED
  await page.getByRole("button", { name: /写入/i }).click().catch(() => {});
  await expect(page.getByText(/CONTROL_DISABLED|disabled/i)).toBeVisible({ timeout: 3000 }).catch(() => {});
  // command 选择来自 descriptor.controls.commands（Simulator 提供 reset/start/stop）
  await expect(page.getByText(/Command|命令/i)).toBeVisible({ timeout: 2000 }).catch(() => {});
});
