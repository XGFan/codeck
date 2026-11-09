import { expect, test } from "@playwright/test";

const RICH_ID = "7f3a9c10-0001-4000-8000-000000000001";

test("iPhone 用的对话页只渲染对话，不带 Connect 与侧栏", async ({ page }) => {
  await page.goto(`/transcript.html?sessionId=${RICH_ID}`);
  await expect(page.getByTestId("transcript-page")).toBeVisible();
  await expect(page.getByTestId("prompt-meta").first()).toBeVisible();
  await expect(page.getByTestId("pane-sessions")).toHaveCount(0);
  await expect(page.getByTestId("header")).toHaveCount(0);
  await expect(page.getByTestId("composer")).toHaveCount(0);
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  await expect(page.getByTestId("fork-button").first()).toBeHidden();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
});
