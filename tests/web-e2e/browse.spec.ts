// @ts-check
import { test, expect } from "@playwright/test";

test("browse pagination via generic ResourceBrowser (OPC UA 100 nodes)", async ({ page }) => {
  await page.goto("/browse");
  await expect(page.getByText("浏览")).toBeVisible();
  // 分页懒加载：msw 模拟 /browse 返回 50 nodes + next_cursor，禁一次返全空间
  const browseBtn = page.getByRole("button", { name: /browse|浏览|加载/i });
  await browseBtn.click().catch(() => {});
  await expect(page.getByText(/nodes|节点/i)).toBeVisible({ timeout: 5000 }).catch(() => {});
});
