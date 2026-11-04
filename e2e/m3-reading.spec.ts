import { expect, test, type Page } from "@playwright/test";
import { composeEnd, composeStart, queriesOf } from "./ime";

const RICH_TITLE = "为 /v1/chat 接口增加令牌桶限流中间件";

async function openRich(page: Page) {
  await page.goto("/");
  await page.getByTestId("session-row").filter({ hasText: RICH_TITLE }).click();
  await expect(page.getByTestId("header").getByRole("heading", { name: RICH_TITLE })).toBeVisible();
  await expect(page.getByTestId("transcript").locator("[data-row]").first()).toBeVisible();
}

const calls = (page: Page) => page.evaluate(() => window.__cvCalls ?? []);

test("⌘F 命中折叠的工具输出：自动展开并高亮，Enter 步进，计数更新，esc 关闭", async ({ page }) => {
  await openRich(page);
  await page.keyboard.press("Meta+f");
  const input = page.getByTestId("find-input");
  await expect(input).toBeFocused();
  // Only present inside a collapsed (and truncated) Bash output.
  await input.fill("case_0007");
  const count = page.getByTestId("find-count");
  await expect(count).toHaveText("1 / 1");
  const hit = page.locator('[data-testid="transcript"] [data-highlighted]');
  await expect(hit).toHaveCount(1);
  await expect(hit.getByTestId("tool-output")).toContainText("case_0007");
  await input.fill("has been updated");
  await expect(count).toHaveText(/^1 \/ [2-9]\d*$/);
  await expect(hit).toHaveCount(1);
  await page.screenshot({ path: process.env.SHOT ?? "test-results/m3-reading.png" });
  const first = await hit.getAttribute("data-anchor");
  await page.keyboard.press("Enter");
  await expect(count).toHaveText(/^2 \//);
  await expect(page.locator('[data-testid="transcript"] [data-highlighted]')).not.toHaveAttribute("data-anchor", first!);
  await page.keyboard.press("Shift+Enter");
  await expect(count).toHaveText(/^1 \//);
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("find")).toHaveCount(0);
  await expect(page.locator('[data-testid="transcript"] [data-highlighted]')).toHaveCount(0);
});

test("j/k 按轮次跳转，大纲高亮当前轮；点击大纲刻度跳转", async ({ page }) => {
  await openRich(page);
  const ticks = page.getByTestId("outline-tick");
  expect(await ticks.count()).toBeGreaterThan(2);
  // Jump to the first turn via the outline, then walk with j / k.
  await ticks.first().click();
  await expect(ticks.first()).toHaveAttribute("data-current", "true");
  await expect(page.locator('[data-testid="transcript"] [data-kind="userPrompt"]').first()).toBeInViewport();
  await page.keyboard.press("j");
  await expect(ticks.nth(1)).toHaveAttribute("data-current", "true");
  await expect(page.locator('[data-testid="transcript"] [data-highlighted]')).toHaveCount(1);
  await page.keyboard.press("k");
  await expect(ticks.first()).toHaveAttribute("data-current", "true");
  const last = ticks.last();
  await last.click();
  await expect(last).toHaveAttribute("data-current", "true");
  await ticks.first().hover();
  await expect(page.getByTestId("outline-tip").first()).toBeVisible();
});

test("j 在输入框内不触发轮次跳转", async ({ page }) => {
  await openRich(page);
  await page.keyboard.press("Meta+f");
  await page.getByTestId("find-input").fill("j");
  await expect(page.getByTestId("find-input")).toHaveValue("j");
});

test("复制：resume 命令、Session ID、消息；在 Finder 中显示", async ({ page }) => {
  await openRich(page);
  await page.getByTestId("header-more").click();
  await page.getByRole("menuitem", { name: "复制 Resume 命令" }).click();
  const id = await page.evaluate(() => window.__cvCalls!.find((c) => c.method === "copyText")!.args[0] as string);
  expect(id).toMatch(/^claude --resume [0-9a-f-]{36}$/);
  const sid = id.replace("claude --resume ", "");

  await page.getByTestId("header-more").click();
  await page.getByRole("menuitem", { name: "复制 Session ID" }).click();
  await expect.poll(async () => (await calls(page)).at(-1)).toEqual({ method: "copyText", args: [sid] });

  await page.getByTestId("header-more").click();
  await page.getByRole("menuitem", { name: "在 Finder 中显示" }).click();
  await expect.poll(async () => (await calls(page)).at(-1)).toEqual({ method: "revealSessionFile", args: [sid, null] });

  const prompt = page
    .locator('[data-testid="transcript"] [data-kind="userPrompt"]')
    .filter({ has: page.getByTestId("copy-button") })
    .first();
  await prompt.hover();
  await prompt.getByTestId("copy-button").click();
  const copied = (await calls(page)).at(-1)!;
  expect(copied.method).toBe("copyText");
  expect(String(copied.args[0]).length).toBeGreaterThan(0);
});

test("⌘F 输入法组字期间不查找，确认后才查找", async ({ page }) => {
  await openRich(page);
  await page.keyboard.press("Meta+f");
  const input = page.getByTestId("find-input");
  await expect(input).toBeFocused();
  await composeStart(input, ["c", "ca", "case"]);
  await expect(input).toHaveValue("case");
  // Longer than the 180 ms debounce: nothing may be queried while composing.
  await page.waitForTimeout(600);
  expect(await queriesOf(page, "findInSession")).toEqual([]);
  await expect(page.getByTestId("find-count")).toHaveText("");
  await composeEnd(input, "case_0007");
  await expect(page.getByTestId("find-count")).toHaveText("1 / 1");
  expect(await queriesOf(page, "findInSession")).toEqual(["case_0007"]);
});
