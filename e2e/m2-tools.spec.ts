import { expect, test, type Locator, type Page } from "@playwright/test";
import { unfoldAll } from "./unfold";

const RICH_TITLE = "为 /v1/chat 接口增加令牌桶限流中间件";

async function openRich(page: Page) {
  await page.goto("/");
  await page.getByTestId("session-row").filter({ hasText: RICH_TITLE }).click();
  await expect(page.getByTestId("header").getByRole("heading", { name: RICH_TITLE })).toBeVisible();
  await unfoldAll(page);
}

/** Scrolls `scroller` by `dy` and resolves after the scroll event (and the virtualizer's re-render); false at the end. */
const scrollBy = (scroller: Locator, dy: number) =>
  scroller.evaluate(
    (e, dy) =>
      new Promise<boolean>((done) => {
        const onScroll = () => requestAnimationFrame(() => done(true));
        e.addEventListener("scroll", onScroll, { once: true });
        const before = e.scrollTop;
        e.scrollTop = before + dy;
        if (e.scrollTop === before) {
          e.removeEventListener("scroll", onScroll);
          done(false);
        }
      }),
    dy,
  );

/** Scrolls the virtualized transcript from the top, opening collapsed tool groups, until `target` is rendered. */
async function reveal(page: Page, target: Locator) {
  const transcript = page.getByTestId("transcript");
  await expect(transcript.locator("[data-row]").first()).toBeVisible();
  // Resolve only once the scroll event has fired: the virtualizer re-renders in its scroll handler, so until
  // then the rows of the previous position (the session opens at the bottom) are still mounted and
  // `target.count()` would match a row that unmounts right after.
  await transcript.evaluate(
    (e) =>
      new Promise<void>((done) => {
        if (e.scrollTop === 0) return done();
        e.addEventListener("scroll", () => done(), { once: true });
        e.scrollTop = 0;
      }),
  );
  for (let i = 0; i < 200 && (await target.count()) === 0; i++) {
    const closed = transcript.locator('[data-testid="tool-group"]:not([data-open]) > button');
    if ((await closed.count()) > 0) {
      const button = (await closed.first().elementHandle())!;
      await button.click();
      await expect.poll(() => button.getAttribute("aria-expanded")).toBe("true");
    } else if (!(await scrollBy(transcript, 400))) break;
  }
  await target.first().scrollIntoViewIfNeeded();
}

test("失败的 Bash 默认收起，展开后渲染 ANSI 颜色", async ({ page }) => {
  await openRich(page);
  const failed = page.locator('[data-tool="Bash"][data-failed]').first();
  await reveal(page, failed);
  await expect(failed.getByTestId("terminal")).toHaveCount(0);
  await failed.getByRole("button").first().click();
  const term = failed.getByTestId("terminal");
  await expect(term).toContainText("unresolved import");
  await expect(failed).toContainText("退出码 101");
  const colored = term.locator("span[style*='color']").filter({ hasText: "error[E0432]" });
  await expect(colored).toBeVisible();
  const color = await colored.evaluate((e) => getComputedStyle(e).color);
  expect(color).not.toBe(await term.evaluate((e) => getComputedStyle(e).color));
});

test("Edit 展开后显示带 +/- 的 diff", async ({ page }) => {
  await openRich(page);
  const edit = page.locator('[data-tool="Edit"]').filter({ hasText: "mod.rs" }).first();
  await reveal(page, edit);
  await edit.getByRole("button").first().click();
  await expect(edit.locator('[data-diff="add"]')).toHaveCount(1);
  await expect(edit.locator('[data-diff="del"]')).toHaveCount(1);
  await expect(edit.locator('[data-diff="add"]')).toContainText("rate_limit::layer()");
  await expect(edit).toContainText("+1");
  await expect(edit).toContainText("−1");
});

test("持久化输出可点击加载完整内容", async ({ page }) => {
  await openRich(page);
  const bash = page.locator('[data-tool="Bash"]').filter({ hasText: "cargo test --workspace" });
  await reveal(page, bash);
  await bash.getByRole("button").first().click();
  await expect(bash).toContainText("来源：tool-results/bd2x9k1.txt");
  await bash.getByRole("button", { name: /加载完整输出（共 180\.0 KB）/ }).click();
  await expect(bash).toContainText("412 passed");
  await expect(bash).toContainText("中间");
  await expect(bash.getByRole("button", { name: /加载完整输出/ })).toHaveCount(0);
});

test("复制 Bash 命令", async ({ page }) => {
  await openRich(page);
  const bash = page.locator('[data-tool="Bash"]').filter({ hasText: "cargo test --workspace" });
  await reveal(page, bash);
  await bash.getByRole("button").first().click();
  await bash.hover();
  await bash.getByRole("button", { name: "复制命令" }).click();
  const calls = await page.evaluate(() => window.__cvCalls ?? []);
  expect(calls.at(-1)).toEqual({ method: "copyText", args: ["cargo test --workspace"] });
});

test("图片缩略图渲染并可放大", async ({ page }) => {
  await openRich(page);
  const read = page.locator('[data-tool="Read"]').filter({ hasText: "grafana-ratelimit.png" });
  await reveal(page, read);
  await read.getByRole("button").first().click();
  const thumb = read.getByTestId("image-thumb").locator("img");
  await expect(thumb).toBeVisible();
  await read.getByTestId("image-thumb").click();
  await expect(page.getByTestId("image-large")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("image-large")).toHaveCount(0);
});

test("AskUserQuestion 单独显示问题并标出你的回答，任务清单显示状态", async ({ page }) => {
  await openRich(page);
  const ask = page.getByTestId("ask").filter({ hasText: "限流超限时应返回什么？" });
  await reveal(page, ask);
  // Shown expanded, outside any tool group.
  await expect(page.locator('[data-testid="tool-group"] [data-testid="ask"]')).toHaveCount(0);
  await expect(ask.getByTestId("ask-question")).toHaveCount(2);
  await expect(ask.locator("[data-chosen]").first()).toContainText("429 + Retry-After");
  await expect(ask.locator("[data-chosen]").nth(1)).toContainText("其他：先放内存，留好换 Redis 的接口");
  await expect(ask.getByTestId("ask-status")).toContainText(/你 · \d\d:\d\d 已回答/);
  await expect(ask.getByTestId("ask-question").first()).toContainText("备注：Retry-After 用秒数");
  // The answer is marked in the card only, not repeated elsewhere.
  await expect(page.getByTestId("transcript").getByText("先放内存，留好换 Redis 的接口")).toHaveCount(1);
  const todo = page.locator('[data-tool="TodoWrite"]');
  await reveal(page, todo);
  await todo.getByRole("button").first().click();
  await expect(todo.getByTestId("checklist").locator("li")).toHaveCount(3);
});

test("AskUserQuestion 选择先聊聊时不显示为失败，标出已答的问题", async ({ page }) => {
  await openRich(page);
  const ask = page.getByTestId("ask").filter({ hasText: "按什么维度限流？" });
  await reveal(page, ask);
  await expect(ask).not.toHaveAttribute("data-failed");
  await expect(ask.getByTestId("ask-status")).toHaveText(/^你 · \d\d:\d\d 选择先聊聊$/);
  await expect(ask.locator("[data-chosen]")).toHaveText("API key");
  await expect(ask).not.toContainText("The user doesn't want to proceed");
});
