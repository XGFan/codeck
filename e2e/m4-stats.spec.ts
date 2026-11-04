import { expect, test, type Page } from "@playwright/test";

/** Picks a time range from the stats' dropdown. */
async function pickRange(page: Page, name: string) {
  await page.getByTestId("range-trigger").click();
  await page.getByRole("menuitemcheckbox", { name, exact: true }).click();
}

test("统计面板渲染卡片与图表", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("nav-stats").click();
  const panel = page.getByTestId("stats");
  await expect(panel.getByRole("heading", { name: "统计" })).toBeVisible();
  await expect(page.getByTestId("pane-sessions")).toBeVisible(); // a dialog over the window, the list stays
  await expect(page.getByTestId("range-trigger")).toHaveText("本周期"); // the default
  await pickRange(page, "最近 30 天");
  await expect(page.getByTestId("stat-card")).toHaveCount(5);
  await expect(page.locator('[data-card="sessions"]')).toContainText("10");
  await expect(page.locator('[data-card="cost"]')).toContainText("$412.37");
  const tokens = page.locator('[data-card="tokens"]');
  await expect(tokens).toContainText("输入");
  await expect(tokens).toContainText("输出");
  await expect(tokens.getByTestId("cache-hit")).toHaveText(/缓存命中率 \d+%/);
  await expect(tokens.getByRole("img", { name: /输入构成：缓存读取 \d+%/ })).toBeVisible();
  await expect(page.getByTestId("daily-chart").locator("canvas").first()).toBeVisible();
  await expect(page.getByTestId("activity-chart").locator("canvas").first()).toBeVisible();
  const ranks = page.getByTestId("session-rank-row");
  await expect(ranks.first()).toBeVisible();
  expect(await ranks.count()).toBeLessThanOrEqual(10);
  await expect(page.locator('[data-card="sessions"]')).not.toContainText("Project");
  await expect(page.getByText("工具调用")).toHaveCount(0);
  await expect(page.getByTestId("stats").getByText("Subagent", { exact: true })).toHaveCount(0);

  await pickRange(page, "最近 7 天");
  await expect(page.getByTestId("stat-card")).toHaveCount(5);
});

test("活跃时段按时间范围自动决定柱宽，没有模式切换", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("nav-stats").click();
  const bucket = page.getByTestId("activity-bucket");
  await expect(bucket).toHaveText("1h"); // 本周期 by default
  await pickRange(page, "最近 30 天");
  await expect(bucket).toHaveText("6h");
  await pickRange(page, "最近 90 天");
  await expect(bucket).toHaveText("1d");
  await pickRange(page, "全部");
  await expect(bucket).toHaveText("1w");
  await expect(page.getByTestId("stats-activity").getByRole("button")).toHaveCount(0);
});

test("Session 排行按金额倒序，点击打开该 Session", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("nav-stats").click();
  await expect(page.getByTestId("stats-project-filter")).toHaveCount(0);
  const ranks = page.getByTestId("session-rank-row");
  const costs = await ranks.evaluateAll((rows) => rows.map((r) => Number(/\$([\d,.]+)/.exec(r.textContent ?? "")![1]!.replace(/,/g, ""))));
  expect(costs).toEqual([...costs].sort((a, b) => b - a));
  const title = (await ranks.first().getAttribute("title"))!;
  await ranks.first().click();
  await expect(page.getByTestId("stats")).toHaveCount(0);
  await expect(page.getByTestId("header").getByRole("heading", { name: title })).toBeVisible();
});

test("每日 token 可在全部、输入、输出间切换：全部为输入、输出两根柱子，其余按模型", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("nav-stats").click();
  const daily = page.getByTestId("stats-daily");
  await expect(daily).toContainText("每日 token");
  await expect(daily).not.toContainText("按模型");
  await expect(daily).not.toContainText("含缓存");
  await expect(daily.getByTestId("legend-item")).toHaveCount(0);
  await daily.getByRole("button", { name: "输出", exact: true }).click();
  await expect(daily).toContainText("按模型");
  await expect(daily.getByTestId("legend-item").first()).toBeVisible();
  await daily.getByRole("button", { name: "全部" }).click();
  await expect(daily.getByTestId("legend-item")).toHaveCount(0);
});

test("点击活跃时段的柱子把 Session 列表筛到那段时间", async ({ page }) => {
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.goto("/");
  await page.getByTestId("nav-stats").click();
  await pickRange(page, "全部");
  await expect(page.getByTestId("activity-bucket")).toHaveText("1w");
  const chart = page.getByTestId("activity-chart");
  await chart.locator("canvas").first().waitFor();
  const box = (await chart.boundingBox())!;
  // Click just above the x axis (grid: left 40, right 8, bottom 22) until a bar is hit; bars sit at
  // fixed fractions of the plot width, and a few may be empty.
  const stats = page.getByTestId("stats");
  for (const f of [0.5, 0.42, 0.58, 0.34, 0.66]) {
    await page.mouse.click(box.x + 40 + (box.width - 48) * f, box.y + box.height - 25);
    if ((await stats.count()) === 0) break;
  }
  await expect(stats).toHaveCount(0);
  const chip = page.getByTestId("drill-chip");
  await expect(chip).toHaveText(/\d+月\d+日–\d+月\d+日/);
  await chip.getByRole("button", { name: "清除筛选" }).click();
  await expect(chip).toHaveCount(0);
});

test("点击模型图例按模型下钻会话列表", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("nav-stats").click();
  const daily = page.getByTestId("stats-daily");
  await daily.getByRole("button", { name: "输出", exact: true }).click();
  await daily.getByTestId("legend-item").filter({ hasText: "opus" }).click();
  await expect(page.getByTestId("stats")).toHaveCount(0);
  await expect(page.getByTestId("session-row").first()).toBeVisible();
  const chip = page.getByTestId("drill-chip");
  await expect(chip).toContainText("模型 claude-opus-4-7");
  const filtered = await page.getByTestId("session-row").count();
  await chip.getByRole("button", { name: "清除筛选" }).click();
  await expect(chip).toHaveCount(0);
  await expect.poll(() => page.getByTestId("session-row").count()).toBeGreaterThanOrEqual(filtered);
});

test("下钻带上统计的时间范围", async ({ page }) => {
  await page.goto("/");
  const review = page.getByTestId("session-row").filter({ hasText: "/review 订单列表页" }); // last active 8 days ago

  const opus = async () => {
    const daily = page.getByTestId("stats-daily");
    await daily.getByRole("button", { name: "输出", exact: true }).click();
    await daily.getByTestId("legend-item").filter({ hasText: "opus" }).click();
  };
  await page.getByTestId("nav-stats").click();
  await pickRange(page, "全部");
  await opus();
  await expect(review).toBeVisible();

  await page.getByTestId("nav-stats").click();
  await pickRange(page, "最近 7 天");
  await opus();
  await expect(page.getByTestId("drill-chip")).toBeVisible();
  await expect(review).toHaveCount(0);
});

test("统计是盖在窗口上的大弹窗：点弹窗外、Esc 或关闭按钮都会关闭", async ({ page }) => {
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.goto("/");
  const stats = page.getByTestId("stats");
  await page.getByTestId("nav-stats").click();
  await expect(stats).toBeVisible();
  const box = (await stats.boundingBox())!;
  expect(box.width).toBeGreaterThan(1200);
  expect(box.height).toBeGreaterThan(800);
  await page.mouse.click(10, 450);
  await expect(stats).toHaveCount(0);

  await page.getByTestId("nav-stats").click();
  await expect(stats).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(stats).toHaveCount(0);

  await page.getByTestId("nav-stats").click();
  await page.getByRole("button", { name: "关闭统计" }).click();
  await expect(stats).toHaveCount(0);
});

test("窗口变高时图表不随之拉高", async ({ page }) => {
  const heights = async () => [
    (await page.getByTestId("daily-chart").boundingBox())!.height,
    (await page.getByTestId("activity-chart").boundingBox())!.height,
  ];
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.goto("/");
  await page.getByTestId("nav-stats").click();
  await page.getByTestId("daily-chart").locator("canvas").first().waitFor();
  const before = await heights();
  await page.setViewportSize({ width: 1400, height: 1400 });
  await expect.poll(heights).toEqual(before);
  expect(before[0]).toBeLessThanOrEqual(260);
});

test("滚动统计内容时，标题与时间范围固定不动", async ({ page }) => {
  await page.setViewportSize({ width: 1400, height: 620 });
  await page.goto("/");
  await page.getByTestId("nav-stats").click();
  await page.getByTestId("daily-chart").locator("canvas").first().waitFor();
  const range = page.getByTestId("range-trigger");
  const before = (await range.boundingBox())!.y;
  const body = page.getByTestId("stats-scroll");
  await body.evaluate((e) => (e.scrollTop = e.scrollHeight));
  await expect.poll(() => body.evaluate((e) => e.scrollTop)).toBeGreaterThan(0);
  expect((await range.boundingBox())!.y).toBe(before);
  await expect(page.getByRole("heading", { name: "统计" })).toBeInViewport();
  await expect(range).toBeInViewport();
});

/** Sets the weekly reset in Settings › 通用 and closes the dialog. */
async function setReset(page: Page, weekday: string, time?: string) {
  await page.getByTestId("settings-button").click();
  await page.getByRole("tab", { name: "通用" }).click();
  await page.getByLabel("重置日").selectOption(weekday);
  if (time) await page.getByLabel("重置时间").fill(time);
  await page.keyboard.press("Escape");
}

test("本周期：按设置里的每周重置时间统计，设置会被记住", async ({ page }) => {
  // A Thursday morning; with a Monday 14:00 reset the cycle began three days ago.
  await page.clock.setFixedTime(new Date("2026-10-15T10:00:00"));
  await page.goto("/");
  await setReset(page, "0", "14:00");
  await page.getByTestId("nav-stats").click();
  const span = page.getByTestId("cycle-span");
  await expect(span).toHaveText("10月12日 周一 14:00 至 10月19日 周一 14:00，还剩 4 天 4 小时");
  await expect(page.getByTestId("stat-card")).toHaveCount(5);
  await expect(page.getByTestId("stats").getByLabel("重置日")).toHaveCount(0);
  await page.keyboard.press("Escape");

  // Thursday 14:00 is still ahead today, so the cycle began last Thursday.
  await setReset(page, "3");
  await page.getByTestId("nav-stats").click();
  await expect(span).toContainText("10月8日 周四 14:00");

  await page.reload();
  await page.getByTestId("settings-button").click();
  await page.getByRole("tab", { name: "通用" }).click();
  await expect(page.getByLabel("重置时间")).toHaveValue("14:00");
  await page.keyboard.press("Escape");
  await page.getByTestId("nav-stats").click();
  await expect(span).toContainText("至 10月15日 周四 14:00，还剩 4 小时 0 分");
});

test("周期与月份可用 ‹ › 前后翻，最多翻到当前；最近 N 天不翻", async ({ page }) => {
  await page.clock.setFixedTime(new Date("2026-10-15T10:00:00"));
  await page.goto("/");
  await setReset(page, "0", "14:00");
  await page.getByTestId("nav-stats").click();
  const trigger = page.getByTestId("range-trigger");
  const span = page.getByTestId("cycle-span");
  const next = page.getByLabel("下一周期", { exact: true });
  await expect(next).toBeDisabled();
  await page.getByLabel("上一周期", { exact: true }).click();
  await expect(trigger).toHaveText("上一周期");
  await expect(span).toHaveText("10月5日 周一 14:00 至 10月12日 周一 14:00");
  await page.getByLabel("上一周期", { exact: true }).click();
  await expect(trigger).toHaveText("9月28日起的周期");
  await next.click();
  await next.click();
  await expect(trigger).toHaveText("本周期");
  await expect(next).toBeDisabled();

  await pickRange(page, "上月");
  await expect(span).toHaveText("2026 年 9 月");
  await page.getByLabel("上一个月", { exact: true }).click();
  await expect(trigger).toHaveText("2026 年 8 月");
  await expect(page.getByTestId("stat-card")).toHaveCount(5);
  await expect(page.locator('[data-card="active"]')).toContainText("/ 31");

  await pickRange(page, "最近 7 天");
  await expect(span).toHaveText("");
  await expect(page.getByLabel("上一周期", { exact: true })).toBeDisabled();
  await expect(next).toBeDisabled();
});

test("切换时间范围时页面不移动：标题栏、卡片与 Token 卡片里的输入输出位置不变", async ({ page }) => {
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.goto("/");
  await page.getByTestId("nav-stats").click();
  const watched = [
    page.getByRole("heading", { name: "统计" }),
    page.getByTestId("range-trigger"),
    page.getByTestId("stat-card").first(),
    page.getByTestId("token-output"),
  ];
  const positions = async () => Promise.all(watched.map(async (l) => (await l.boundingBox())!).map(async (b) => [(await b).x, (await b).y]));
  const before = await positions();
  for (const name of ["最近 30 天", "全部", "上月", "本周期"]) {
    await pickRange(page, name);
    await expect(page.getByTestId("token-output")).toBeVisible();
    expect(await positions()).toEqual(before);
  }
  await expect(page.getByTestId("cycle-span")).toContainText("还剩");
});

test("卡片高度随自身内容，不被同行更高的卡片撑高", async ({ page }) => {
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.goto("/");
  await page.getByTestId("nav-stats").click();
  await page.getByTestId("daily-chart").locator("canvas").first().waitFor();
  // Blank space below each card's last child: its 12px bottom padding plus the border, nothing more.
  const slack = await page.getByTestId("stats").locator("section").evaluateAll((cards) =>
    cards.map((c) => c.getBoundingClientRect().bottom - c.lastElementChild!.getBoundingClientRect().bottom),
  );
  expect(slack.length).toBeGreaterThanOrEqual(3);
  for (const s of slack) expect(s).toBeLessThanOrEqual(14);
});
