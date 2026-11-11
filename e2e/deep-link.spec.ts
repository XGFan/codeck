import { expect, test, type Page } from "@playwright/test";
import { RICH_TITLE } from "./helpers";

const openUrl = (page: Page, url: string) => page.evaluate((u) => window.__cvMock!.openUrl(u), url);
const heading = (page: Page) => page.getByTestId("header").getByRole("heading");

test("codeck://session/<id> 打开这个 Session，盖在上面的统计面板关掉", async ({ page }) => {
  await page.goto("/");
  const id = await page.getByTestId("session-row").filter({ hasText: RICH_TITLE }).getAttribute("data-session-id");
  await page.getByTestId("nav-stats").click();
  await expect(page.getByTestId("stats")).toBeVisible();

  await openUrl(page, `codeck://session/${id}`);
  await expect(heading(page)).toHaveText(RICH_TITLE);
  await expect(page.getByTestId("stats")).toHaveCount(0);
  await expect(page.locator(`[data-testid=session-row][data-session-id="${id}"]`)).toHaveAttribute(
    "aria-selected",
    "true",
  );
});

test("认不出的链接不改变当前页面", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("session-row").filter({ hasText: RICH_TITLE }).click();
  await expect(heading(page)).toHaveText(RICH_TITLE);
  for (const url of ["codeck://session/abc", "codeck://new?cwd=/tmp&prompt=hi", "https://example.com"]) {
    await openUrl(page, url);
  }
  await expect(heading(page)).toHaveText(RICH_TITLE);
  await expect(page.getByTestId("transcript-error")).toHaveCount(0);
});

test("索引还在扫描时，链接里的 Session 先显示等待，扫描完仍找不到才报错", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() =>
    window.__cvMock!.emit("indexStatus", {
      phase: "scanning",
      filesTotal: 10,
      filesDone: 1,
      bytesTotal: 100,
      bytesDone: 10,
      sessionsTotal: 1,
      textReady: false,
      error: null,
    }),
  );
  await openUrl(page, "codeck://session/00000000-0000-4000-8000-000000000000");
  await expect(page.getByTestId("transcript-scanning")).toHaveText("正在更新索引，完成后显示这个 Session");

  await page.evaluate(() =>
    window.__cvMock!.emit("indexStatus", {
      phase: "idle",
      filesTotal: 10,
      filesDone: 10,
      bytesTotal: 100,
      bytesDone: 100,
      sessionsTotal: 1,
      textReady: true,
      error: null,
    }),
  );
  await expect(page.getByTestId("transcript-error")).toBeVisible();
});
