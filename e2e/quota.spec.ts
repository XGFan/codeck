import { expect, test, type Page } from "@playwright/test";
import { openStats } from "./helpers";

const planCalls = (page: Page) =>
  page.evaluate(() => ({
    calls: window.__cvPlan!.calls,
    refreshCalls: window.__cvPlan!.refreshCalls,
  }));

test("侧栏圆环与浮层：显示各项额度与重置时刻，点「打开统计」进入统计页且两处共用一份数据", async ({ page }) => {
  await page.clock.setFixedTime(new Date("2026-11-25T14:11:00"));
  await page.goto("/");

  const ring = page.getByTestId("nav-quota");
  await expect(ring).toBeVisible();
  await expect(ring).toHaveAttribute("aria-label", "额度：最高一项已用 45%");
  await expect(ring).toHaveAttribute("data-tone", "normal");

  await ring.click();
  const pop = page.getByTestId("quota-popover");
  await expect(pop).toBeVisible();
  await expect(pop.getByTestId("quota-plan")).toHaveText("Max");
  const popMeters = pop.getByTestId("quota-meter");
  await expect(popMeters).toHaveCount(3);
  await expect(popMeters.nth(0)).toContainText("当前 5 小时");
  await expect(popMeters.nth(0)).toContainText("16%");
  await expect(popMeters.nth(0)).toContainText("今天 16:31");
  await expect(popMeters.nth(1)).toContainText("本周 · 全部模型");
  await expect(popMeters.nth(1)).toContainText("31%");
  await expect(popMeters.nth(1)).toContainText("11月28日 周六 14:11");
  await expect(popMeters.nth(2)).toContainText("本周 · Fable");
  await expect(popMeters.nth(2)).toContainText("45%");
  await expect(pop.getByTestId("quota-updated")).toHaveText("14:11 更新");

  // 打开统计后，统计页顶部额度条与侧栏共用同一个 query，不重复拉取。
  await pop.getByTestId("quota-open-stats").click();
  await expect(pop).toHaveCount(0);
  const strip = page.getByTestId("quota-strip");
  await expect(strip).toBeVisible();
  await expect(strip).toHaveAttribute("data-state", "ok");
  await expect(strip.getByTestId("quota-plan")).toHaveText("Max");
  await expect(strip.getByTestId("quota-meter")).toHaveCount(3);
  await expect(strip.getByTestId("quota-meter").nth(0)).toContainText("时间已过");
  await expect(strip.getByTestId("quota-updated")).toHaveText("14:11 更新");
  expect(await planCalls(page)).toEqual({ calls: 1, refreshCalls: 0 });
});

test("刷新后统计页与侧栏圆环同时更新，≥80% / ≥100% 变色并写明状态；重取失败保留上次数据", async ({ page }) => {
  const nowMs = new Date("2026-11-25T14:11:00").getTime();
  await page.clock.setFixedTime(new Date(nowMs));
  await page.goto("/");
  await expect(page.getByTestId("nav-quota")).toBeVisible();
  await openStats(page);

  const strip = page.getByTestId("quota-strip");
  await expect(strip).toHaveAttribute("data-state", "ok");

  // 改为 ≥80%（琥珀色「快用完了」）、≥100%（红色「已用完」）、以及无 resetsMs（不画竖线）。
  await page.evaluate((now) => {
    window.__cvPlan!.set({
      plan: "pro",
      fetchedMs: now + 60_000,
      limits: [
        { kind: "session", model: null, percent: 86, resetsMs: now + 48 * 60_000 },
        { kind: "weeklyAll", model: null, percent: 100, resetsMs: now + 125 * 60_000 },
        { kind: "weeklyScoped", model: "Sonnet", percent: 24, resetsMs: null },
      ],
    });
  }, nowMs);

  await strip.getByTestId("quota-refresh").click();
  await expect(strip.getByTestId("quota-plan")).toHaveText("Pro");
  await expect(strip.getByTestId("quota-updated")).toHaveText("14:12 更新");

  const meters = strip.getByTestId("quota-meter");
  await expect(meters.nth(0)).toHaveAttribute("data-tone", "warn");
  await expect(meters.nth(0)).toContainText("86%");
  await expect(meters.nth(0)).toContainText("快用完了 · 48 分后重置");
  await expect(meters.nth(0).getByTestId("quota-tick")).toHaveCount(1);

  await expect(meters.nth(1)).toHaveAttribute("data-tone", "full");
  await expect(meters.nth(1)).toContainText("100%");
  await expect(meters.nth(1)).toContainText("已用完 · 2 小时 5 分后重置");
  await expect(meters.nth(1).getByTestId("quota-tick")).toHaveCount(1);

  await expect(meters.nth(2)).toHaveAttribute("data-tone", "normal");
  await expect(meters.nth(2)).toContainText("24%");
  await expect(meters.nth(2).getByTestId("quota-tick")).toHaveCount(0);
  await expect(meters.nth(2)).not.toContainText("后重置");

  // 侧栏圆环同时更新为最高项（100%，full）且仅多发一次 refresh=true。
  await page.keyboard.press("Escape");
  const ring = page.getByTestId("nav-quota");
  await expect(ring).toHaveAttribute("aria-label", "额度：最高一项已用 100%");
  await expect(ring).toHaveAttribute("data-tone", "full");
  expect(await planCalls(page)).toEqual({ calls: 2, refreshCalls: 1 });

  // 后续刷新失败：仍保持 ok（沿用上次数据），浮层与统计页都写「读取失败，显示的是 HH:MM 的数据」。
  await page.evaluate(() => window.__cvPlan!.setError("读取订阅额度超时"));
  await ring.click();
  const pop = page.getByTestId("quota-popover");
  await pop.getByTestId("quota-refresh").click();
  await expect(pop.getByTestId("quota-updated")).toHaveText("读取失败，显示的是 14:12 的数据");
  await expect(pop.getByTestId("quota-meter").nth(0)).toContainText("86%");
  await pop.getByTestId("quota-open-stats").click();
  await expect(strip.getByTestId("quota-updated")).toHaveText("读取失败，显示的是 14:12 的数据");
  expect(await planCalls(page)).toEqual({ calls: 3, refreshCalls: 2 });
});

test("loading 状态：侧栏显示统计图标，统计页顶部三条空条，加载完成后切换为圆环与额度条", async ({ page }) => {
  await page.addInitScript(() => {
    window.__cvPlanPreset = { hold: true };
  });
  await page.goto("/");

  await expect(page.getByTestId("nav-stats")).toBeVisible();
  await expect(page.getByTestId("nav-quota")).toHaveCount(0);
  await page.getByTestId("nav-stats").click();

  const strip = page.getByTestId("quota-strip");
  await expect(strip).toBeVisible();
  await expect(strip).toHaveAttribute("data-state", "loading");
  await expect(strip.getByTestId("quota-meter-loading")).toHaveCount(3);
  await expect(strip).toContainText("正在读取");

  await page.evaluate(() => window.__cvPlan!.release());
  await expect(strip).toHaveAttribute("data-state", "ok");
  await expect(strip.getByTestId("quota-meter")).toHaveCount(3);
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("nav-quota")).toBeVisible();
});

test("failed 状态：侧栏退回统计图标，统计页显示原因与重试且不影响下方统计；重试成功后恢复", async ({ page }) => {
  await page.addInitScript(() => {
    window.__cvPlanPreset = { error: "未找到 claude" };
  });
  await page.goto("/");

  await expect(page.getByTestId("nav-stats")).toBeVisible();
  await expect(page.getByTestId("nav-quota")).toHaveCount(0);
  await page.getByTestId("nav-stats").click();

  const strip = page.getByTestId("quota-strip");
  await expect(strip).toHaveAttribute("data-state", "failed");
  await expect(strip).toContainText("读不到订阅额度");
  await expect(strip).toContainText("未找到 claude");
  await expect(page.getByTestId("stats-usage")).toBeVisible();

  await page.evaluate(() => window.__cvPlan!.reset());
  await strip.getByTestId("quota-retry").click();
  await expect(strip).toHaveAttribute("data-state", "ok");
  expect(await planCalls(page)).toEqual({ calls: 1, refreshCalls: 1 });
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("nav-quota")).toBeVisible();
});

test("none 状态：侧栏退回统计图标，统计页整条额度不显示", async ({ page }) => {
  await page.addInitScript(() => {
    window.__cvPlanPreset = { value: null };
  });
  await page.goto("/");

  await expect(page.getByTestId("nav-stats")).toBeVisible();
  await expect(page.getByTestId("nav-quota")).toHaveCount(0);
  await openStats(page);
  await expect(page.getByTestId("quota-strip")).toHaveCount(0);
  await expect(page.getByTestId("stats-usage")).toBeVisible();
});

test("周期跟随订阅的每周重置（含设置说明行），切换为 none 时清掉锚并回到设置的值", async ({ page }) => {
  const nowMs = new Date("2026-11-25T14:11:00").getTime();
  // Monday 14:00 minus 200ms: stored unrounded in localStorage, rounded to 14:00 when displayed.
  const anchorMs = new Date("2026-11-30T13:59:59.800").getTime();
  await page.addInitScript(
    ({ now, anchor }) => {
      window.__cvPlanPreset = {
        value: {
          plan: "max",
          fetchedMs: now,
          limits: [
            { kind: "session", model: null, percent: 20, resetsMs: now + 2 * 3_600_000 },
            { kind: "weeklyAll", model: null, percent: 35, resetsMs: anchor },
          ],
        },
      };
    },
    { now: nowMs, anchor: anchorMs },
  );
  await page.clock.setFixedTime(new Date(nowMs));
  await page.goto("/");
  await expect(page.getByTestId("nav-quota")).toBeVisible();

  // 先在设置里配置不同的统计周期（周三 09:00），并确认说明行写明了当前跟随的订阅重置时刻。
  await page.getByTestId("settings-button").click();
  await page.getByRole("tab", { name: "通用" }).click();
  await page.getByLabel("重置日").selectOption("2");
  await page.getByLabel("重置时间").fill("09:00");
  await expect(page.getByTestId("set-cycle-plan-note")).toHaveText(
    "已跟随订阅的每周重置（周一 14:00）；读不到额度时用这里的设置",
  );
  await page.keyboard.press("Escape");

  // 统计页「本周期」跟随订阅的每周重置，且 localStorage 保存的是四舍五入到分钟的稳定锚值。
  await openStats(page);
  const span = page.getByTestId("cycle-span");
  await expect(span).toHaveText("11月23日 周一 14:00 至 11月30日 周一 14:00，还剩 4 天 23 小时 · 跟随订阅的每周重置");
  const roundedAnchorMs = new Date("2026-11-30T14:00:00.000").getTime();
  expect(await page.evaluate(() => localStorage.getItem("codeck.planCycleAnchor"))).toBe(String(roundedAnchorMs));

  // 变为 none 并刷新：额度条消失，锚被清掉，本周期回到设置里的周三 09:00。
  await page.evaluate(() => window.__cvPlan!.set(null));
  await page.getByTestId("quota-strip").getByTestId("quota-refresh").click();
  await expect(page.getByTestId("quota-strip")).toHaveCount(0);
  await expect(span).toHaveText("11月25日 周三 09:00 至 12月2日 周三 09:00，还剩 6 天 18 小时");
  expect(await page.evaluate(() => localStorage.getItem("codeck.planCycleAnchor"))).toBeNull();

  await page.keyboard.press("Escape");
  await expect(page.getByTestId("nav-stats")).toBeVisible();
  await expect(page.getByTestId("nav-quota")).toHaveCount(0);

  // 设置里的说明行不再出现。
  await page.getByTestId("settings-button").click();
  await page.getByRole("tab", { name: "通用" }).click();
  await expect(page.getByTestId("set-cycle-plan-note")).toHaveCount(0);
});

test("侧栏隐藏时圆环移入顶部工具栏可打开浮层；读取失败时沿用已保存的周期锚", async ({ page }) => {
  const nowMs = new Date("2026-11-25T14:11:00").getTime();
  const anchorMs = new Date("2026-11-30T14:00:00").getTime();
  await page.clock.setFixedTime(new Date(nowMs));
  await page.goto("/");
  await expect(page.getByTestId("nav-quota")).toBeVisible();

  // 隐藏侧栏后，顶部工具栏里的圆环也能点开浮层并打开统计。
  await page.keyboard.press("Control+Meta+L");
  const toolbarRing = page.getByTestId("toolbar").getByTestId("nav-quota");
  await expect(toolbarRing).toBeVisible();
  await toolbarRing.click();
  const pop = page.getByTestId("quota-popover");
  await expect(pop).toBeVisible();
  await pop.getByTestId("quota-open-stats").click();
  await expect(page.getByTestId("stats")).toBeVisible();

  // 写入已保存的周期锚，在下次启动读取额度失败时仍沿用该锚。
  await page.addInitScript(
    ({ anchor }) => {
      localStorage.setItem("codeck.planCycleAnchor", String(anchor));
      window.__cvPlanPreset = { error: "读取订阅额度超时" };
    },
    { anchor: anchorMs },
  );
  await page.reload();
  await expect(page.getByTestId("nav-stats")).toBeVisible();
  await openStats(page);
  await expect(page.getByTestId("quota-strip")).toHaveAttribute("data-state", "failed");
  await expect(page.getByTestId("cycle-span")).toHaveText(
    "11月23日 周一 14:00 至 11月30日 周一 14:00，还剩 4 天 23 小时 · 跟随订阅的每周重置",
  );
});

test("窗口失焦时不轮询，重新聚焦且过期后重取一次，前台每 5 分钟轮询", async ({ page }) => {
  await page.clock.install({ time: new Date("2026-11-25T14:11:00") });
  await page.goto("/");
  await page.clock.runFor(100);
  await expect(page.getByTestId("nav-quota")).toBeVisible();
  expect(await planCalls(page)).toEqual({ calls: 1, refreshCalls: 0 });

  // 模拟 Tauri 窗口失焦但仍可见：推进 10 分钟不应触发轮询。
  await page.evaluate(() => {
    Object.defineProperty(document, "hasFocus", { value: () => false, configurable: true });
    window.dispatchEvent(new Event("blur"));
  });
  await page.clock.fastForward("10:00");
  await page.clock.runFor(100);
  expect(await planCalls(page)).toEqual({ calls: 1, refreshCalls: 0 });

  // 回到前台且数据已过期（> 60 秒）：重取一次。
  await page.evaluate(() => {
    Object.defineProperty(document, "hasFocus", { value: () => true, configurable: true });
    window.dispatchEvent(new Event("focus"));
  });
  await page.clock.runFor(100);
  await expect.poll(() => planCalls(page)).toEqual({ calls: 2, refreshCalls: 0 });

  // 60 秒内失焦再聚焦（未过期）：不重取。
  await page.clock.runFor(10_000);
  await page.evaluate(() => {
    Object.defineProperty(document, "hasFocus", { value: () => false, configurable: true });
    window.dispatchEvent(new Event("blur"));
  });
  await page.evaluate(() => {
    Object.defineProperty(document, "hasFocus", { value: () => true, configurable: true });
    window.dispatchEvent(new Event("focus"));
  });
  await page.clock.runFor(100);
  expect(await planCalls(page)).toEqual({ calls: 2, refreshCalls: 0 });

  // 持续在前台 5 分钟：自动轮询一次。
  await page.clock.fastForward("05:05");
  await page.clock.runFor(100);
  await expect.poll(() => planCalls(page)).toEqual({ calls: 3, refreshCalls: 0 });
});
