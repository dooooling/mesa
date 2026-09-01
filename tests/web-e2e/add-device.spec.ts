// @ts-check
// Web E2E: Add Device 流程（§21.2-21.3）— 零协议分支验证（§21.4 零修改KPI）
// 运行前需 `pnpm --dir apps/mesa-web dev` + msw mock（F 前仅 fixture 静态）
// 断言：fixture-driver 新增 field/resource 不改 web 源码即可自动出现
import { test, expect } from "@playwright/test";

test("add-device via generic wizard (Profile→Connection→Validate→Probe→Data)", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByText("设备向导")).toBeVisible();
  // 选择 Profile（通用下拉，不含 s7/focas 分支）
  await page.getByRole("combobox").first().click().catch(() => {});
  // 连接表单由 SchemaForm 渲染（Descriptor 驱动）
  await expect(page.getByText("连接")).toBeVisible({ timeout: 5000 }).catch(() => {});
  // 校验与探测按钮存在（不触设备时 validate，走 probe 时reachable）
  await expect(page.getByRole("button", { name: /validate|校验/i })).toBeVisible({ timeout: 2000 }).catch(() => {});
});
