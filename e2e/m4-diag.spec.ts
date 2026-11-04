import { expect, test } from "@playwright/test";

test("⌘, 打开设置，格式兼容性渲染 mock 计数", async ({ page }) => {
  await page.goto("/");
  await page.keyboard.press("Meta+,");
  const dialog = page.getByRole("dialog");
  await expect(dialog).toBeVisible();
  await expect(dialog.getByTestId("set-version")).toContainText("0.1.0");

  await dialog.getByRole("tab", { name: "格式兼容性" }).click();
  await expect(dialog.getByTestId("diag-card-failed")).toContainText("3 行");
  await expect(dialog.getByTestId("diag-card-tools")).toContainText("1 个");
  await expect(dialog.getByTestId("diag-entry-types")).toContainText("frame-link");
  await expect(dialog.getByTestId("diag-tools")).toContainText("ScheduleWakeup");
  await expect(dialog.getByTestId("diag-versions")).toContainText("2.1.88");
  await expect(dialog.getByTestId("diag-failures")).toContainText("5e1c0b7a-0006");
  await expect(dialog.getByTestId("diag-integrity")).toContainText("2");
  await page.screenshot({ path: process.env.SHOT ?? "test-results/m4-diag.png" });

  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
});

test("更改数据目录调用 API 并更新显示，恢复默认", async ({ page }) => {
  await page.goto("/");
  await page.keyboard.press("Meta+,");
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("tab", { name: "数据" }).click();
  await expect(dialog.getByTestId("set-root-path")).toHaveText("/Users/dev/.claude");
  await expect(dialog.getByTestId("set-root-source")).toContainText("默认");

  await dialog.getByTestId("set-root-input").fill("/tmp/fixture-root");
  await dialog.getByTestId("set-root-apply").click();
  await expect(dialog.getByTestId("set-root-path")).toHaveText("/tmp/fixture-root");
  await expect(dialog.getByTestId("set-root-source")).toContainText("设置");

  await dialog.getByTestId("set-root-reset").click();
  await expect(dialog.getByTestId("set-root-source")).toContainText("默认");

  await dialog.getByTestId("set-rebuild").click();
  await expect(dialog.getByTestId("set-index-status")).toContainText("已索引");
});

test("更改数据目录后清空已选 Session", async ({ page }) => {
  await page.goto("/");
  await page.locator("[data-testid=session-row]:not([data-pending])").first().click();
  await expect(page.getByTestId("transcript")).toBeVisible();
  await page.keyboard.press("Meta+,");
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("tab", { name: "数据" }).click();
  await dialog.getByTestId("set-root-input").fill("/tmp/fixture-root");
  await dialog.getByTestId("set-root-apply").click();
  await expect(dialog.getByTestId("set-root-path")).toHaveText("/tmp/fixture-root");
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("transcript-empty")).toBeVisible();
});
