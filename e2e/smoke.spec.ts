import { expect, test } from "@playwright/test";

test("两栏布局与统计、设置入口可见", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("pane-sessions")).toBeVisible();
  await expect(page.getByTestId("pane-conversation")).toBeVisible();
  await expect(page.getByTestId("nav-stats")).toBeVisible();
  await expect(page.getByTestId("settings-button")).toBeVisible();
});

test("右侧对话头部不再有搜索按钮", async ({ page }) => {
  await page.goto("/");
  await page.locator("[data-testid=session-row]:not([data-pending])").first().click();
  await expect(page.getByTestId("header")).toBeVisible();
  await expect(page.getByTestId("header")).not.toContainText("搜索");
  await expect(page.getByTestId("search-button")).toBeVisible();
});
