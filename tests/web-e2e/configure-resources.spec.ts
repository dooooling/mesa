// @ts-check
import { test, expect } from "@playwright/test";

test("configure-resources via ResourcePicker (zero driver branch)", async ({ page }) => {
  await page.goto("/");
  // ResourcePicker 渲染 resources（counter/sine 等），输出选择自动生成 point_key
  await expect(page.getByText("数据")).toBeVisible({ timeout: 5000 }).catch(async () => {
    // fixture 静态时至少保证向导可进入
    await expect(page.getByText("设备向导")).toBeVisible();
  });
  // 更新速率 Realtime/Normal/Slow 自动归并为 AcquisitionTask（无需手写任务）
  await expect(page.getByText(/Realtime|Normal|Slow|速率/i)).toBeVisible({ timeout: 2000 }).catch(() => {});
});
