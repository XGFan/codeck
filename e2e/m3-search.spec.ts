import { expect, test, type Page } from "@playwright/test";
import { composeEnd, composeStart, escWhileComposing, queriesOf } from "./ime";

const RICH_TITLE = "为 /v1/chat 接口增加令牌桶限流中间件";

async function openSearch(page: Page) {
  await page.goto("/");
  await expect(page.getByTestId("session-row").first()).toBeVisible();
  await page.keyboard.press("Meta+k");
  await expect(page.getByTestId("search")).toBeVisible();
}

const input = (page: Page) => page.locator("[cmdk-input]");
const sessions = (page: Page) => page.getByTestId("search-session");
const selected = (page: Page) => page.locator('[data-testid="search"] [cmdk-item][data-selected="true"]');

test("⌘K 打开，空输入列出最近的 Session，再按一次关闭；⌘P 不再打开", async ({ page }) => {
  await openSearch(page);
  await expect(sessions(page).first()).toContainText(RICH_TITLE);
  await expect(page.getByTestId("search-group")).toHaveCount(0);
  await page.keyboard.press("Meta+k");
  await expect(page.getByTestId("search")).toBeHidden();
  await page.keyboard.press("Meta+p");
  await expect(page.getByTestId("search")).toBeHidden();
  await page.getByTestId("search-button").click();
  await expect(page.getByTestId("search")).toBeVisible();
});

test("输入后 Session 段即时出现，对话内容按 Session 分组并高亮；关闭后再打开保留输入", async ({ page }) => {
  await openSearch(page);
  await input(page).fill("令牌桶");
  await expect(sessions(page)).toHaveCount(1);
  await expect(sessions(page).locator("mark")).toHaveText(["令牌桶"]);
  const groups = page.getByTestId("search-group");
  await expect(groups.first()).toBeVisible();
  await expect(groups.first().locator("mark").first()).toBeVisible();
  await expect(page.getByTestId("search-summary")).toContainText("个 Session");
  await expect(selected(page)).toHaveAttribute("data-testid", "search-session");
  await page.screenshot({ path: process.env.SHOT ?? "test-results/m3-search.png" });
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("search")).toHaveCount(0);
  await page.keyboard.press("Meta+k");
  await expect(input(page)).toHaveValue("令牌桶");
  // Selected, so typing replaces it.
  await page.keyboard.type("zod");
  await expect(input(page)).toHaveValue("zod");
});

test("Session 与 Project 支持短语和排除词", async ({ page }) => {
  await openSearch(page);
  await input(page).fill("lumen -redis");
  await expect(page.getByTestId("search-project")).toHaveText([/lumen-api/]);
  await expect(sessions(page).filter({ hasText: "排查 Redis 连接池泄漏" })).toHaveCount(0);
  await expect(sessions(page).filter({ hasText: RICH_TITLE })).toHaveCount(1);
  await input(page).fill('"表单校验"');
  await expect(sessions(page)).toHaveText([/迁移表单校验到 zod/]);
});

test("⇥ 跳到对话内容段，Enter 打开命中并定位", async ({ page }) => {
  await openSearch(page);
  await input(page).fill("令牌桶");
  await expect(page.getByTestId("search-hit").first()).toBeVisible();
  await page.keyboard.press("Tab");
  await expect(selected(page)).toHaveAttribute("data-testid", "search-hit");
  await page.keyboard.press("Shift+Tab");
  await expect(selected(page)).toHaveAttribute("data-testid", "search-session");
  await page.keyboard.press("Tab");
  await expect(page.getByTestId("search")).toContainText("打开并定位");
  const title = await page.getByTestId("search-group").first().locator("span.font-semibold").innerText();
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("search")).toHaveCount(0);
  await expect(page.getByTestId("header").getByRole("heading", { name: title })).toBeVisible();
});

test("Enter 打开 Session；不在当前 Project 筛选里时清除筛选", async ({ page }) => {
  await openSearch(page);
  await input(page).fill("orbit-web");
  await expect(selected(page)).toHaveAttribute("data-testid", "search-project");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("project-chip")).toContainText("orbit-web");
  await page.keyboard.press("Meta+k");
  await input(page).fill("redis-leak");
  await expect(sessions(page)).toHaveText([/排查 Redis 连接池泄漏/]);
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("project-chip")).toHaveCount(0);
  await expect(page.getByTestId("header").getByRole("heading", { name: "排查 Redis 连接池泄漏" })).toBeVisible();
  await expect(page.getByTestId("session-row").filter({ hasText: "排查 Redis 连接池泄漏" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
});

test("⌘↵ 把 Project 设为搜索范围，三段都只在其中；输入为空时 ⌫ 移除范围", async ({ page }) => {
  await openSearch(page);
  await input(page).fill("lumen");
  await expect(selected(page)).toHaveAttribute("data-testid", "search-project");
  await page.keyboard.press("Meta+Enter");
  await expect(page.getByTestId("search-scope")).toContainText("lumen-api");
  await expect(input(page)).toHaveValue("");
  await expect(input(page)).toHaveAttribute("placeholder", "在 lumen-api 中搜索");
  // Includes the session started in orbit-web that edited lumen-api.
  await expect(sessions(page).filter({ hasText: "迁移表单校验到 zod" })).toHaveCount(1);
  await expect(sessions(page).filter({ hasText: "优化首页首屏加载" })).toHaveCount(0);
  // "已" appears in both lumen-api and orbit-web conversations.
  await input(page).fill("已");
  await expect(page.getByTestId("search-project")).toHaveCount(0);
  const groups = page.getByTestId("search-group");
  await expect(groups.first()).toBeVisible();
  for (const g of await groups.all()) await expect(g).toContainText("lumen-api");
  await expect(groups.filter({ hasText: "优化首页首屏加载" })).toHaveCount(0);
  const lastScope = await page.evaluate(() => window.__cvCalls!.filter((c) => c.method === "search").at(-1)!.args[1]);
  expect(lastScope).toBe("-Users-dev-Developer-lumen-api");
  // Backspace only removes the scope once the input is empty.
  await page.keyboard.press("Backspace");
  await expect(input(page)).toHaveValue("");
  await expect(page.getByTestId("search-scope")).toHaveCount(1);
  await page.keyboard.press("Backspace");
  await expect(page.getByTestId("search-scope")).toHaveCount(0);
  await expect(sessions(page).filter({ hasText: "优化首页首屏加载" })).toHaveCount(1);
});

test("点「只搜这个 Project」后焦点仍在输入框，可以直接打字", async ({ page }) => {
  await openSearch(page);
  await input(page).fill("lumen");
  await page
    .getByTestId("search-project")
    .getByRole("button", { name: /只搜这个 Project/ })
    .click();
  await expect(page.getByTestId("search-scope")).toContainText("lumen-api");
  await page.keyboard.type("zod");
  await expect(input(page)).toHaveValue("zod");
  await page.getByRole("button", { name: "移除 Project 范围" }).click();
  await page.keyboard.type("!");
  await expect(input(page)).toHaveValue("zod!");
});

test("能搜到 AskUserQuestion 的回答，打开后定位到问答", async ({ page }) => {
  await openSearch(page);
  await input(page).fill("留好换 Redis");
  const hit = page.getByTestId("search-hit").first();
  await expect(hit).toContainText("你");
  await hit.click();
  await expect(page.getByTestId("search")).toHaveCount(0);
  await expect(page.getByTestId("ask").filter({ hasText: "留好换 Redis" })).toBeInViewport();
});

test("Subagent 内的命中带 Subagent 标签，点击后跳转", async ({ page }) => {
  await openSearch(page);
  await input(page).fill("cors");
  const tag = page.getByTestId("tag-subagent").first();
  await expect(tag).toContainText(/Subagent · \S+/);
  await tag.click();
  await expect(page.getByTestId("search")).toHaveCount(0);
  await expect(page.getByTestId("header")).toBeVisible();
});

test("无结果时两段各自说明；只有排除词时提示需要包含词", async ({ page }) => {
  await openSearch(page);
  await input(page).fill("zzz_no_such_text");
  await expect(page.getByTestId("search-session-empty")).toContainText("没有标题、Project 或分支匹配的 Session");
  await expect(page.getByTestId("search-empty")).toContainText("没有找到 “zzz_no_such_text”");
  await input(page).fill("-redis");
  await expect(page.getByTestId("search")).toContainText("至少需要一个包含词");
});

test("输入法组字期间不过滤也不发起搜索，确认后按确认的文字查找", async ({ page }) => {
  await openSearch(page);
  const all = await sessions(page).count();
  await composeStart(input(page), ["l", "ling", "ling pai tong"]);
  await expect(input(page)).toHaveValue("ling pai tong");
  // Longer than the 200 ms debounce: nothing may be queried while composing.
  await page.waitForTimeout(600);
  expect(await queriesOf(page, "search")).toEqual([]);
  await expect(sessions(page)).toHaveCount(all);
  await composeEnd(input(page), "令牌桶");
  await expect(page.getByTestId("search-hit").first()).toBeVisible();
  await expect(sessions(page)).toHaveCount(1);
  expect(await queriesOf(page, "search")).toEqual(["令牌桶"]);
});

test("组字时按 Esc 只取消候选词，不关闭弹窗", async ({ page }) => {
  await openSearch(page);
  await composeStart(input(page), ["l", "ling"]);
  await escWhileComposing(input(page));
  await expect(page.getByTestId("search")).toBeVisible();
  await composeEnd(input(page), "");
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("search")).toBeHidden();
});
