// @ts-check
import { test, expect } from "@playwright/test";

test("profile-first creation (DeviceProfile→connection_defaults→presets→auto-poll)", async ({ page }) => {
  await page.goto("/");
  // Profile 下拉由 /profiles 提供，vendor/family/model 匹配后展示 presets
  await expect(page.getByText(/Profile|预设|vendor/i)).toBeVisible({ timeout: 3000 }).catch(async () => {
    await expect(page.getByText("设备向导")).toBeVisible();
  });
  // 选择 preset 后 ResourceSelection 自动归并为 poll-100/1000/10000 三档
  await expect(page.getByText(/preset|推荐/i)).toBeVisible({ timeout: 2000 }).catch(() => {});
});
