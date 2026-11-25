import { expect, test, type Page } from "@playwright/test";
import { openStats } from "./helpers";

/** Picks a time range from the stats' dropdown. */
async function pickRange(page: Page, name: string) {
  await page.getByTestId("range-trigger").click();
  await page.getByRole("menuitemcheckbox", { name, exact: true }).click();
}

/** Sets the weekly reset in Settings › 通用 and closes the dialog. */
async function setReset(page: Page, weekday: string, time?: string) {
  await page.getByTestId("settings-button").click();
  await page.getByRole("tab", { name: "通用" }).click();
  await page.getByLabel("重置日").selectOption(weekday);
  if (time) await page.getByLabel("重置时间").fill(time);
  await page.keyboard.press("Escape");
}

test("统计页新布局：左用量、右使用频率与用量走势，已移除旧概览卡片与排行", async ({ page }) => {
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.goto("/");
  await openStats(page);

  const panel = page.getByTestId("stats");
  await expect(panel.getByRole("heading", { name: "统计" })).toBeVisible();
  await expect(page.getByTestId("pane-sessions")).toBeVisible();
  await expect(page.getByTestId("range-trigger")).toHaveText("本周期");

  // Old overview cards, session ranking, and activity chart are gone.
  await expect(page.getByTestId("stat-card")).toHaveCount(0);
  await expect(panel.getByText("Session 排行")).toHaveCount(0);
  await expect(panel.getByText("活跃时段")).toHaveCount(0);
  await expect(panel.getByText("活跃天数")).toHaveCount(0);

  // Left column: three figures with daily averages, bar chart, Cost by model, input composition.
  const usage = page.getByTestId("stats-usage");
  await expect(usage).toBeVisible();
  await expect(usage.getByTestId("usage-bucket")).toHaveText("1d");
  const costBtn = usage.getByTestId("metric-cost");
  const inputBtn = usage.getByTestId("metric-input");
  const outputBtn = usage.getByTestId("metric-output");
  await expect(costBtn).toHaveAttribute("aria-pressed", "true");
  await expect(costBtn).toContainText("日均");
  await expect(inputBtn).toContainText("日均");
  await expect(outputBtn).toContainText("日均");
  await expect(usage.getByTestId("usage-chart").locator("canvas").first()).toBeVisible();

  await inputBtn.click();
  await expect(inputBtn).toHaveAttribute("aria-pressed", "true");
  await expect(costBtn).toHaveAttribute("aria-pressed", "false");
  await outputBtn.click();
  await expect(outputBtn).toHaveAttribute("aria-pressed", "true");
  await costBtn.click();
  await expect(costBtn).toHaveAttribute("aria-pressed", "true");

  await expect(usage.getByTestId("model-cost-row").first()).toBeVisible();
  await expect(usage.getByTestId("cache-hit")).toHaveText(/缓存命中率 \d+\.\d%/);
  await expect(
    usage.getByTestId("input-breakdown").getByRole("img", { name: /输入构成：缓存读取 \d+%/ }),
  ).toBeVisible();

  // Switching to 全部 widens bars to 1w.
  await pickRange(page, "全部");
  await expect(usage.getByTestId("usage-bucket")).toHaveText("1w");
});

test("各时间范围的使用频率形态与用量走势显示规则", async ({ page }) => {
  await page.clock.setFixedTime(new Date("2026-11-25T14:11:00"));
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.goto("/");
  await openStats(page);

  const freq = page.getByTestId("stats-frequency");
  const trend = page.getByTestId("stats-trend");

  // Current cycle (non-midnight start): 8 chronological day rows, no start marker, future boxes noted in legend.
  await expect(freq).toHaveAttribute("data-mode", "hourly");
  await expect(freq.getByTestId("freq-hour-row")).toHaveCount(8);
  await expect(freq.locator('[data-start="true"]')).toHaveCount(0);
  await expect(freq).toContainText("空框是还没到的时间");
  await expect(freq.getByTestId("freq-subtitle")).toContainText("个小时有请求");
  await expect(trend).toBeVisible();
  await expect(trend.getByTestId("trend-chart").locator("canvas").first()).toBeVisible();
  await expect(trend.getByTestId("trend-subtitle")).toContainText("到现在");
  await expect(trend.getByTestId("trend-prev-legend")).toHaveText("上一周期");

  // Previous cycle: 8 chronological day rows (completed, no future boxes) + completed trend ("共").
  await pickRange(page, "上一周期");
  await expect(freq).toHaveAttribute("data-mode", "hourly");
  await expect(freq.getByTestId("freq-hour-row")).toHaveCount(8);
  await expect(freq).not.toContainText("空框是还没到的时间");
  await expect(trend).toBeVisible();
  await expect(trend.getByTestId("trend-subtitle")).toContainText("共");

  // Last 7 days: 8 chronological day rows + trend vs previous 7 days.
  await pickRange(page, "最近 7 天");
  await expect(freq).toHaveAttribute("data-mode", "hourly");
  await expect(freq.getByTestId("freq-hour-row")).toHaveCount(8);
  await expect(trend).toBeVisible();
  await expect(trend.getByTestId("trend-prev-legend")).toHaveText("前 7 天");

  // Last month & last 30 days: single calendar, no trend; first cell & 1st of month show M/D.
  await pickRange(page, "上月");
  await expect(freq).toHaveAttribute("data-mode", "calendar");
  await expect(freq.getByTestId("freq-calendar")).toBeVisible();
  await expect(freq.locator('[data-testid="freq-day"][data-day="2026-10-01"] b')).toHaveText("10/1");
  await expect(freq.locator('[data-testid="freq-day"][data-day="2026-10-02"] b')).toHaveText("2");
  await expect(freq.getByTestId("freq-subtitle")).toContainText("天有请求");
  await expect(trend).toHaveCount(0);

  await pickRange(page, "最近 30 天");
  await expect(freq).toHaveAttribute("data-mode", "calendar");
  await expect(freq.locator('[data-testid="freq-day"][data-day="2026-10-26"] b')).toHaveText("10/26");
  await expect(freq.locator('[data-testid="freq-day"][data-day="2026-11-01"] b')).toHaveText("11/1");
  await expect(freq.locator('[data-testid="freq-day"][data-day="2026-11-02"] b')).toHaveText("2");
  await expect(trend).toHaveCount(0);

  // Last 90 days: 2-column mini month blocks, no trend.
  await pickRange(page, "最近 90 天");
  await expect(freq).toHaveAttribute("data-mode", "months");
  await expect(freq.getByTestId("freq-months")).toHaveAttribute("data-columns", "2");
  await expect(freq.getByTestId("freq-year")).toHaveCount(0);
  await expect(trend).toHaveCount(0);

  // All: 4-column mini month blocks grouped by year, no trend.
  await pickRange(page, "全部");
  await expect(freq).toHaveAttribute("data-mode", "months");
  await expect(freq.getByTestId("freq-months")).toHaveAttribute("data-columns", "4");
  expect(await freq.getByTestId("freq-year").count()).toBeGreaterThanOrEqual(2);
  await expect(trend).toHaveCount(0);
});

test("切换范围等待新数据时，标题栏切到新范围，下方保持上一范围的完整画面且屏蔽下钻与预估", async ({ page }) => {
  await page.clock.setFixedTime(new Date("2026-11-25T14:11:00"));
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.goto("/");
  await openStats(page);

  const stats = page.getByTestId("stats");
  const freq = page.getByTestId("stats-frequency");
  const trend = page.getByTestId("stats-trend");
  await expect(freq).toHaveAttribute("data-mode", "hourly");
  await expect(trend.getByTestId("trend-estimate-legend")).toBeVisible();
  await expect(trend.getByTestId("trend-prev-legend")).toHaveText("上一周期");

  // Hold the next getStats response so placeholderData stays active while switching to 上月.
  await page.evaluate(() => window.__cvMock!.holdStats(true));
  await pickRange(page, "上月");

  // Header reflects the newly chosen range immediately.
  await expect(page.getByTestId("range-trigger")).toHaveText("上月");
  await expect(page.getByTestId("cycle-span")).toHaveText("2026 年 10 月");
  await expect(stats).toHaveAttribute("data-placeholder", "true");

  // Panels keep the previous range's paired snapshot (7×24 hourly grid + trend + gray reference curve),
  // while estimate, diff text, and drill-down are disabled until new data arrives.
  await expect(freq).toHaveAttribute("data-mode", "hourly");
  await expect(trend).toBeVisible();
  await expect(trend.getByTestId("trend-prev-legend")).toHaveText("上一周期");
  await expect(trend.getByTestId("trend-subtitle")).not.toContainText("比上一周期");
  await expect(trend.getByTestId("trend-estimate-legend")).toHaveCount(0);
  await page.locator('[data-testid="model-cost-row"][data-family="opus"]').click();
  await expect(stats).toBeVisible();

  // Release the response: panels switch to 上月's calendar and trend disappears.
  await page.evaluate(() => window.__cvMock!.releaseStats());
  await expect(stats).not.toHaveAttribute("data-placeholder", "true");
  await expect(freq).toHaveAttribute("data-mode", "calendar");
  await expect(trend).toHaveCount(0);
});

test("预估额度只在本周期且额度可用时出现，上一周期 / 最近 7 天 / none 时不出现", async ({ page }) => {
  await page.clock.setFixedTime(new Date("2026-11-25T14:11:00"));
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.goto("/");
  await openStats(page);

  const trend = page.getByTestId("stats-trend");
  const estimateLegend = page.getByTestId("trend-estimate-legend");
  await expect(estimateLegend).toBeVisible();
  await expect(estimateLegend).toHaveText("预估额度 = 已用 Cost ÷ 本周已用 31%");
  const ymax = Number(await trend.getAttribute("data-ymax"));
  const est = Number(await trend.getAttribute("data-estimate"));
  expect(est).toBeGreaterThan(0);
  expect(ymax).toBeGreaterThanOrEqual(est);

  await pickRange(page, "上一周期");
  await expect(trend).toBeVisible();
  await expect(estimateLegend).toHaveCount(0);
  expect(await trend.getAttribute("data-estimate")).toBeNull();

  await pickRange(page, "最近 7 天");
  await expect(trend).toBeVisible();
  await expect(estimateLegend).toHaveCount(0);

  // Back to current cycle, switch plan usage to none: estimate disappears while trend stays.
  await pickRange(page, "本周期");
  await expect(estimateLegend).toBeVisible();
  await page.evaluate(() => window.__cvPlan!.set(null));
  await page.getByTestId("quota-strip").getByTestId("quota-refresh").click();
  await expect(page.getByTestId("quota-strip")).toHaveCount(0);
  await expect(trend).toBeVisible();
  await expect(estimateLegend).toHaveCount(0);
});

test("下钻：模型行按模型族下钻并带时间范围，月历格与日柱按天下钻，小时格与周柱不下钻", async ({ page }) => {
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.goto("/");
  const review = page.getByTestId("session-row").filter({ hasText: "/review 订单列表页" }); // last active 8 days ago
  const stats = page.getByTestId("stats");

  // 1. Model row drill with time range: 全部 includes the 8-day-old opus Session, 最近 7 天 excludes it.
  await openStats(page);
  await pickRange(page, "全部");
  await expect(stats).not.toHaveAttribute("data-placeholder", "true");
  await expect(page.getByTestId("usage-bucket")).toHaveText("1w");
  await page.locator('[data-testid="model-cost-row"][data-family="opus"]').click();
  await expect(stats).toHaveCount(0);
  const chip = page.getByTestId("drill-chip");
  await expect(chip).toContainText("模型 opus");
  await expect(review).toBeVisible();
  expect(await page.getByTestId("session-row").count()).toBeGreaterThan(0);

  await openStats(page);
  await pickRange(page, "最近 7 天");
  await expect(stats).not.toHaveAttribute("data-placeholder", "true");
  await expect(page.getByTestId("trend-prev-legend")).toHaveText("前 7 天");
  await page.locator('[data-testid="model-cost-row"][data-family="opus"]').click();
  await expect(stats).toHaveCount(0);
  await expect(chip).toContainText("模型 opus");
  await expect(page.getByTestId("session-row").first()).toBeVisible();
  await expect(review).toHaveCount(0);
  await chip.getByRole("button", { name: "清除筛选" }).click();
  await expect(chip).toHaveCount(0);

  // 2. Fable model row drill sets the family drill chip.
  await openStats(page);
  await expect(stats).not.toHaveAttribute("data-placeholder", "true");
  await page.locator('[data-testid="model-cost-row"][data-family="fable"]').click();
  await expect(stats).toHaveCount(0);
  await expect(chip).toContainText("模型 fable");
  await chip.getByRole("button", { name: "清除筛选" }).click();

  // 3. Calendar cell drill (today's cell).
  await openStats(page);
  await pickRange(page, "本月");
  await expect(stats).not.toHaveAttribute("data-placeholder", "true");
  const todayCell = page.locator('[data-testid="freq-day"][data-today="true"]');
  await expect(todayCell).toBeVisible();
  await todayCell.click();
  await expect(stats).toHaveCount(0);
  await expect(chip).toHaveText(/\d+月\d+日/);
  await chip.getByRole("button", { name: "清除筛选" }).click();

  // 4. Hourly cells and weekly bars do not drill.
  await openStats(page);
  await expect(stats).not.toHaveAttribute("data-placeholder", "true");
  await page.getByTestId("freq-hour").first().click();
  await expect(stats).toBeVisible();

  await pickRange(page, "全部");
  await expect(stats).not.toHaveAttribute("data-placeholder", "true");
  await expect(page.getByTestId("usage-bucket")).toHaveText("1w");
  const usageChart = page.getByTestId("usage-chart");
  await usageChart.locator("canvas").first().waitFor();
  const weekBox = (await usageChart.boundingBox())!;
  await page.mouse.click(weekBox.x + weekBox.width * 0.5, weekBox.y + weekBox.height - 30);
  await expect(stats).toBeVisible();

  // 5. Daily bar drill (on 上一周期's 1d chart).
  await pickRange(page, "上一周期");
  await expect(stats).not.toHaveAttribute("data-placeholder", "true");
  await expect(page.getByTestId("usage-bucket")).toHaveText("1d");
  await usageChart.locator("canvas").first().waitFor();
  const dayBox = (await usageChart.boundingBox())!;
  for (const f of [0.25, 0.4, 0.55, 0.7, 0.15, 0.85]) {
    await page.mouse.click(dayBox.x + 42 + (dayBox.width - 50) * f, dayBox.y + dayBox.height - 28);
    if ((await stats.count()) === 0) break;
  }
  await expect(stats).toHaveCount(0);
  await expect(chip).toHaveText(/\d+月\d+日/);
  await chip.getByRole("button", { name: "清除筛选" }).click();
  await expect(chip).toHaveCount(0);
});

test("统计是盖在窗口上的大弹窗：点弹窗外、Esc 或关闭按钮都会关闭", async ({ page }) => {
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.goto("/");
  const stats = page.getByTestId("stats");
  await openStats(page);
  await expect(stats).toBeVisible();
  const box = (await stats.boundingBox())!;
  expect(box.width).toBeGreaterThan(1200);
  expect(box.height).toBeGreaterThan(800);
  await page.mouse.click(10, 450);
  await expect(stats).toHaveCount(0);

  await openStats(page);
  await expect(stats).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(stats).toHaveCount(0);

  await openStats(page);
  await page.getByRole("button", { name: "关闭统计" }).click();
  await expect(stats).toHaveCount(0);
});

test("滚动统计内容时，标题与时间范围固定不动", async ({ page }) => {
  await page.setViewportSize({ width: 1400, height: 540 });
  await page.goto("/");
  await openStats(page);
  await page.getByTestId("usage-chart").locator("canvas").first().waitFor();
  const range = page.getByTestId("range-trigger");
  const before = (await range.boundingBox())!.y;
  const body = page.getByTestId("stats-scroll");
  await body.evaluate((e) => (e.scrollTop = e.scrollHeight));
  await expect.poll(() => body.evaluate((e) => e.scrollTop)).toBeGreaterThan(0);
  expect((await range.boundingBox())!.y).toBe(before);
  await expect(page.getByRole("heading", { name: "统计" })).toBeInViewport();
  await expect(range).toBeInViewport();
});

test("本周期：读不到订阅额度时按设置里的每周重置时间统计，设置会被记住", async ({ page }) => {
  await page.addInitScript(() => {
    window.__cvPlanPreset = { value: null };
  });
  await page.clock.setFixedTime(new Date("2026-10-15T10:00:00"));
  await page.goto("/");
  await setReset(page, "0", "14:00");
  await openStats(page);
  const span = page.getByTestId("cycle-span");
  await expect(span).toHaveText("10月12日 周一 14:00 至 10月19日 周一 14:00，还剩 4 天 4 小时");
  await expect(page.getByTestId("stats-usage")).toBeVisible();
  await expect(page.getByTestId("stats").getByLabel("重置日")).toHaveCount(0);
  await page.keyboard.press("Escape");

  await setReset(page, "3");
  await openStats(page);
  await expect(span).toContainText("10月8日 周四 14:00");

  await page.reload();
  await page.getByTestId("settings-button").click();
  await page.getByRole("tab", { name: "通用" }).click();
  await expect(page.getByLabel("重置时间")).toHaveValue("14:00");
  await page.keyboard.press("Escape");
  await openStats(page);
  await expect(span).toContainText("至 10月15日 周四 14:00，还剩 4 小时 0 分");
});

test("周期与月份可用 ‹ › 前后翻，最多翻到当前；最近 N 天不翻", async ({ page }) => {
  await page.addInitScript(() => {
    window.__cvPlanPreset = { value: null };
  });
  await page.clock.setFixedTime(new Date("2026-10-15T10:00:00"));
  await page.goto("/");
  await setReset(page, "0", "14:00");
  await openStats(page);
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
  await expect(page.getByTestId("stats-frequency")).toHaveAttribute("data-mode", "calendar");

  await pickRange(page, "最近 7 天");
  await expect(span).toHaveText("");
  await expect(page.getByLabel("上一周期", { exact: true })).toBeDisabled();
  await expect(next).toBeDisabled();
});

test("切换时间范围时标题栏与 ‹ › 位置不变，最长标签完整显示", async ({ page }) => {
  await page.clock.setFixedTime(new Date("2027-01-13T10:00:00"));
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.goto("/");
  await openStats(page);
  const range = page.getByRole("group", { name: "时间范围" });
  const trigger = page.getByTestId("range-trigger");
  const watched = [
    page.getByRole("heading", { name: "统计" }),
    trigger,
    range.locator('[aria-label^="上一"]'),
    range.locator('[aria-label^="下一"]'),
    page.getByTestId("stats-usage"),
  ];
  const positions = async () =>
    Promise.all(watched.map(async (l) => (await l.boundingBox())!).map(async (b) => [(await b).x, (await b).y]));
  const before = await positions();
  for (const name of ["最近 30 天", "全部", "上月", "本周期"]) {
    await pickRange(page, name);
    await expect(page.getByTestId("stats-usage")).toBeVisible();
    expect(await positions()).toEqual(before);
  }
  await expect(page.getByTestId("cycle-span")).toContainText("还剩");

  const prev = page.getByLabel("上一周期", { exact: true });
  await prev.click();
  await expect(trigger).toHaveText("上一周期");
  expect(await positions()).toEqual(before);
  await prev.click();
  await expect(trigger).toHaveText("12月26日起的周期");
  expect(await positions()).toEqual(before);
  expect(await trigger.locator("span").evaluate((l) => l.scrollWidth <= l.clientWidth)).toBe(true);
});
