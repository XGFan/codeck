import { expect, test, type Page } from "@playwright/test";

const RICH_TITLE = "为 /v1/chat 接口增加令牌桶限流中间件";

async function openRich(page: Page) {
  await page.goto("/");
  const row = page.getByTestId("session-row").filter({ hasText: RICH_TITLE });
  await row.click();
  await expect(page.getByTestId("header").getByRole("heading", { name: RICH_TITLE })).toBeVisible();
  return (await row.getAttribute("data-session-id"))!;
}

/** Filters the session list by a Project picked in the search dialog. */
async function filterProject(page: Page, name: string) {
  await page.keyboard.press("Meta+K");
  await page.getByTestId("search").getByRole("combobox").fill(name);
  await page.getByTestId("search-project").filter({ hasText: name }).click();
  await expect(page.getByTestId("search")).toBeHidden();
}

test("在搜索里选择 Project 后会话列表被过滤，清除筛选恢复全部", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("session-row").filter({ hasText: RICH_TITLE })).toBeVisible();
  await filterProject(page, "orbit-web");
  await expect(page.getByTestId("project-chip")).toContainText("orbit-web");
  await expect(page.getByTestId("list-title")).toHaveText("orbit-web");
  await expect(page.getByTestId("session-row").filter({ hasText: "迁移表单校验到 zod" })).toBeVisible();
  await expect(page.getByTestId("session-row").filter({ hasText: RICH_TITLE })).toHaveCount(0);
  await page.getByRole("button", { name: "清除 Project 筛选" }).click();
  await expect(page.getByTestId("project-chip")).toHaveCount(0);
  await expect(page.getByTestId("session-row").filter({ hasText: RICH_TITLE })).toBeVisible();
});

test("改动过其他 Project 的 Session 也出现在那个 Project 下；行上不显示 Project", async ({ page }) => {
  await page.goto("/");
  // Started in orbit-web, also edited lumen-api.
  const zod = page.getByTestId("session-row").filter({ hasText: "迁移表单校验到 zod" });
  await expect(zod).not.toContainText("orbit-web");
  await filterProject(page, "lumen-api");
  await expect(page.getByTestId("session-row").filter({ hasText: RICH_TITLE })).toBeVisible();
  await expect(zod).toBeVisible();
});

test("打开会话后渲染标题栏与消息", async ({ page }) => {
  await openRich(page);
  await expect(page.getByTestId("live-state")).toHaveText("工作中");
  await expect(page.getByTestId("header-more")).toBeVisible();
  await expect(page.locator('[data-kind="userPrompt"]').first()).toBeVisible();
  await expect(page.locator('[data-kind="assistant"]').first()).toBeVisible();
  await expect(page.getByTestId("tool-call").first()).toBeVisible();
});

test("实时追加消息后新内容出现且停留在底部", async ({ page }) => {
  const id = await openRich(page);
  const scroller = page.getByTestId("transcript");
  await expect.poll(() => scroller.evaluate((e) => e.scrollHeight - e.scrollTop - e.clientHeight)).toBeLessThan(60);

  await page.evaluate((sid) => {
    window.__cvMock!.appendNode(sid, {
      id: "e2e-new",
      timestampMs: Date.now(),
      hidden: false,
      inherited: false,
      body: { kind: "assistant", messageId: "m-e2e", model: null, usage: null, isApiError: false, blocks: [{ kind: "text", text: "E2E 追加的新消息" }] },
    });
    window.__cvMock!.emit("sessionsChanged", { changed: [sid], removed: [], projectsChanged: false });
  }, id);

  await expect(page.getByText("E2E 追加的新消息")).toBeVisible();
  await expect.poll(() => scroller.evaluate((e) => e.scrollHeight - e.scrollTop - e.clientHeight)).toBeLessThan(60);
});

test("离开底部时出现新消息提示", async ({ page }) => {
  const id = await openRich(page);
  const scroller = page.getByTestId("transcript");
  await scroller.evaluate((e) => (e.scrollTop = 0));
  await expect.poll(() => scroller.evaluate((e) => e.scrollTop)).toBeLessThan(5);
  await page.evaluate((sid) => {
    window.__cvMock!.appendNode(sid, {
      id: "e2e-new2",
      timestampMs: Date.now(),
      hidden: false,
      inherited: false,
      body: { kind: "assistant", messageId: "m-e2e2", model: null, usage: null, isApiError: false, blocks: [{ kind: "text", text: "另一条新消息" }] },
    });
    window.__cvMock!.emit("sessionsChanged", { changed: [sid], removed: [], projectsChanged: false });
  }, id);
  await expect(page.getByTestId("new-messages")).toBeVisible();
  await page.getByTestId("new-messages").click();
  await expect(page.getByText("另一条新消息")).toBeVisible();
});

test("等待作答的 Session 显示琥珀色点", async ({ page }) => {
  await page.goto("/");
  const dot = page.getByTestId("session-row").filter({ hasText: "优化首页首屏加载" }).getByTestId("live-dot");
  await expect(dot).toHaveAttribute("data-state", "waiting");
  await expect(dot).toHaveAttribute("title", "等待作答");
  await page.getByTestId("session-row").filter({ hasText: "优化首页首屏加载" }).click();
  await expect(page.getByTestId("live-state")).toHaveText("等待作答");
});

test("setLive 切换进行中标记", async ({ page }) => {
  const id = await openRich(page);
  const dot = page.getByTestId("session-row").filter({ hasText: RICH_TITLE }).getByTestId("live-dot");
  await expect(dot).toHaveAttribute("data-state", "busy");
  await page.evaluate((sid) => window.__cvMock!.setLive(sid, { status: "idle", rawStatus: "idle", waitingFor: null, pid: null, source: "recentWrite" }), id);
  await expect(dot).toHaveAttribute("data-state", "idle");
  await expect(page.getByTestId("live-state")).toHaveText("空闲");
  await page.evaluate((sid) => window.__cvMock!.setLive(sid, null), id);
  await expect(dot).toHaveCount(0);
  await expect(page.getByTestId("live-state")).toContainText("已结束");
});

test("显示系统消息后出现隐藏节点", async ({ page }) => {
  await openRich(page);
  const hidden = page.getByText("可用技能：review、simplify、security-review");
  await expect(hidden).toHaveCount(0);
  await page.getByTestId("header-more").click();
  await page.getByRole("menuitemcheckbox", { name: "显示系统消息" }).click();
  // The refetch with hidden nodes may land after our scroll while the view is still pinned to the bottom.
  const transcript = page.getByTestId("transcript");
  await expect
    .poll(async () => {
      await transcript.evaluate((e) => (e.scrollTop = 0));
      return hidden.isVisible();
    })
    .toBe(true);
});

test("斜杠命令只显示一个斜杠，命令名高亮", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("session-row").filter({ hasText: "/review 订单列表页" }).click();
  const transcript = page.getByTestId("transcript");
  const name = transcript.getByTestId("command-name").first();
  await expect(name).toHaveText("/review");
  await expect(name.locator("..")).toHaveText("/review 订单列表页");
  // Highlighted like the terminal: the command token is coloured differently from its arguments.
  const [cmdColor, argColor] = await name.evaluate((e) => [getComputedStyle(e).color, getComputedStyle(e.parentElement!).color]);
  expect(cmdColor).not.toBe(argColor);
});

test("本地命令的输出去掉标签和控制码，按命令输出显示", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("session-row").filter({ hasText: "/review 订单列表页" }).click();
  const transcript = page.getByTestId("transcript");
  await expect(transcript.getByTestId("command-name").last()).toHaveText("/rename");
  const output = transcript.getByTestId("command-output");
  await expect(output).toHaveText("命令输出Session renamed to: 订单列表页审查");
});

test("触摸板下拉不会把整个页面拉出空白", async ({ page }) => {
  await page.goto("/");
  // Rubber-banding itself is a trackpad gesture Playwright cannot drive; the root opt-out is what prevents it.
  expect(await page.evaluate(() => getComputedStyle(document.documentElement).overscrollBehaviorY)).toBe("none");
});

test("Session 列表可收起，刷新后仍保持", async ({ page }) => {
  await openRich(page);
  const header = page.getByTestId("header");
  const toolbar = (await page.getByTestId("toolbar").boundingBox())!;
  // The toolbar has its own strip above the list header; the title sits below it.
  expect((await page.getByTestId("list-title").boundingBox())!.y).toBeGreaterThanOrEqual(toolbar.y + toolbar.height);
  const before = (await header.boundingBox())!;

  await page.keyboard.press("Control+Meta+L");
  await expect(page.getByTestId("pane-sessions")).toBeHidden();
  const after = (await header.boundingBox())!;
  expect(after.width).toBeGreaterThan(before.width + 300);
  expect((await header.getByRole("heading").boundingBox())!.x).toBeGreaterThan(toolbar.x + toolbar.width);

  await page.reload();
  await expect(page.getByTestId("pane-sessions")).toBeHidden();
  await page.getByRole("button", { name: "显示 Session 列表" }).click();
  await expect(page.getByTestId("pane-sessions")).toBeVisible();
});

test("滚动条只在滚动时显示", async ({ page }) => {
  await page.setViewportSize({ width: 1400, height: 240 });
  await page.goto("/");
  const list = page.getByRole("listbox", { name: "Session 列表" });
  await expect(page.getByTestId("session-row").first()).toBeVisible();
  const thumb = () => list.evaluate((e) => e.style.getPropertyValue("--scroll-thumb"));
  expect(await thumb()).toBe("");
  await list.evaluate((e) => (e.scrollTop = 40));
  await expect.poll(() => list.evaluate((e) => e.scrollTop)).toBeGreaterThan(0);
  await expect.poll(thumb).not.toBe("");
  await expect.poll(thumb, { timeout: 3000 }).toBe("");
});

test("Session 列表收起时，在搜索里选择 Project 会自动展开列表", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "隐藏 Session 列表" }).click();
  await expect(page.getByTestId("pane-sessions")).toBeHidden();
  await filterProject(page, "orbit-web");
  await expect(page.getByTestId("pane-sessions")).toBeVisible();
  await expect(page.getByTestId("session-row").filter({ hasText: "迁移表单校验到 zod" })).toBeVisible();
});

test("头部显示 Cost，悬停可看按模型明细；其余统计在 ⓘ 的悬停卡片里", async ({ page }) => {
  await openRich(page);
  const cost = page.getByTestId("header").getByTestId("session-cost");
  await expect(cost).toHaveText(/^Cost \$\d+\.\d{2}$/);
  const details = page.getByTestId("session-details");
  await expect(details).toBeHidden();
  await page.getByTestId("header").getByRole("button", { name: "更多统计" }).hover();
  await expect(details).toBeVisible();
  await expect(details).toContainText("工具调用");
  const title = (await cost.getAttribute("title"))!;
  expect(title).toContain("claude-sonnet-5-5");
  expect(title).toContain("claude-haiku-4-5");
});

test("Session 列表可按 Cost 排序", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "排序方式" }).click();
  await page.getByRole("menuitem", { name: "Cost" }).click();
  const values = async () =>
    (await page.getByTestId("row-cost").allTextContents()).map((c) => (c.startsWith("<") ? 0.005 : Number(c.replace(/[$,]/g, ""))));
  // The list refetches after the sort changes; wait for the new order.
  await expect.poll(async () => {
    const v = await values();
    return v.length > 3 && v.every((x, i) => i === 0 || v[i - 1] >= x);
  }).toBe(true);
  const costs = await page.getByTestId("row-cost").allTextContents();
  // A session whose model has no price shows $0 (the untitled one has no model).
  expect(costs).toContain("$0");
});
