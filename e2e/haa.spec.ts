import { expect, test, type Page } from "@playwright/test";
import type { HaaWorker } from "../src/features/haa/protocol";
import { disableAutoConnect, pick } from "./connect-helpers";
import { RICH_TITLE } from "./helpers";

const REDIS_TITLE = "排查 Redis 连接池泄漏";
const REDIS_ID = "91d5be38-0004-4000-8000-000000000004";
const RICH_ID = "7f3a9c10-0001-4000-8000-000000000001";

const haaProbe = (page: Page) =>
  page.evaluate(() => {
    const { watching, watchStarts, ready } = window.__cvHaa!;
    return { watching, watchStarts, ready };
  });

const emitWorkers = (page: Page, workers: HaaWorker[]) => page.evaluate((list) => window.__cvHaa!.emit(list), workers);

const endWatch = (page: Page, message: string) => page.evaluate((msg) => window.__cvHaa!.end(msg), message);

const nextFrames = (page: Page) =>
  page.evaluate(() => new Promise<void>((r) => requestAnimationFrame(() => requestAnimationFrame(() => r()))));

async function waitReady(page: Page) {
  await expect(page.getByTestId("transcript").locator("[data-row]").first()).toBeVisible();
  await expect.poll(() => haaProbe(page).then((p) => p.ready)).toBe(true);
  await nextFrames(page);
}

async function open(page: Page, title: string) {
  await page.getByTestId("session-row").filter({ hasText: title }).click();
  await expect(page.getByTestId("header").getByRole("heading", { name: title })).toBeVisible();
}

const connectHeightVar = (page: Page) =>
  page.getByTestId("transcript").evaluate((el) => {
    let cur: HTMLElement | null = el as HTMLElement;
    while (cur) {
      const v = cur.style.getPropertyValue("--connect-h");
      if (v) return v;
      cur = cur.parentElement;
    }
    return "";
  });

async function assertAlignedNoOverflow(page: Page) {
  const bar = page.getByTestId("haa-workers");
  const stats = await bar.evaluate((el) => {
    const rows = Array.from(el.querySelectorAll<HTMLElement>('[data-testid="haa-worker"]'));
    return {
      barOverflow: el.scrollWidth > el.clientWidth,
      rowOverflows: rows.map((r) => r.scrollWidth > r.clientWidth),
      rowHeights: rows.map((r) => Math.round(r.getBoundingClientRect().height)),
    };
  });
  expect(stats.barOverflow).toBe(false);
  expect(stats.rowOverflows.every((v) => !v)).toBe(true);
  if (stats.rowHeights.length > 1) {
    expect(new Set(stats.rowHeights).size).toBe(1);
  }
}

// The Live Session of these tests can be Connected: left on, 自动 Connect connects it a moment after it opens, which
// moves the status bar into the Connect panel (and collapses it) under a test that expects it not connected.
test.beforeEach(({ page }) => disableAutoConnect(page));

test("历史（非 Live）Session 不启动 watch", async ({ page }) => {
  await page.goto("/");
  await open(page, REDIS_TITLE);
  await waitReady(page);
  expect(await haaProbe(page)).toMatchObject({ watching: false, watchStarts: 0, ready: true });
  await expect(page.getByTestId("haa-workers")).toHaveCount(0);
});

test("Live 未 Connect：启动 watch、过滤 owner、表格折叠与展开、切到非 Live Session 停掉并清空", async ({ page }) => {
  await page.goto("/");
  await open(page, RICH_TITLE);
  await expect.poll(() => haaProbe(page)).toMatchObject({ watching: true, watchStarts: 1 });
  await expect(page.getByTestId("haa-workers")).toHaveCount(0);

  // 仅推入其他 Session 的 haa Worker 时不显示状态条。
  await emitWorkers(page, [{ id: "w-other", owner: REDIS_ID, name: "stranger", state: "running", tokens: 999 }]);
  await nextFrames(page);
  await expect(page.getByTestId("haa-workers")).toHaveCount(0);

  // 1 个本 Session 的 Worker：只显示 owner 匹配的，撑开 `--connect-h`，无 haa-toggle，haa-count 为 1/1。
  const now = Date.now();
  await emitWorkers(page, [
    {
      id: "w-own-1",
      owner: RICH_ID,
      name: "alpha",
      state: "running",
      model: "agy/gemini-4-neon-high",
      tokens: 18400,
      ctx: 37.4,
      title: "修复限流边界",
      activity: "cargo test",
      startedAt: now - 15_000,
    },
    { id: "w-other", owner: REDIS_ID, name: "stranger", state: "running", tokens: 999 },
  ]);

  const bar = page.getByTestId("haa-workers");
  await expect(bar).toBeVisible();
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  await expect(page.getByTestId("haa-toggle")).toHaveCount(0);
  await expect(page.getByTestId("haa-count")).toContainText("1/1");
  await expect(bar).toContainText("alpha");
  await expect(bar).toContainText("运行中");
  await expect(bar).toContainText("18.4K");
  await expect(bar).toContainText("37%");
  await expect(bar).toContainText("修复限流边界");
  await expect(bar).not.toContainText("cargo test");
  await expect(bar).not.toContainText("stranger");
  const taskCell = page.getByTestId("haa-worker").first().locator('[title*="正在做：cargo test"]');
  await expect(taskCell).toHaveCount(1);
  await expect.poll(() => connectHeightVar(page)).not.toBe("");
  await assertAlignedNoOverflow(page);

  // 2 个 Worker：两行都显示，没有 haa-toggle。
  await emitWorkers(page, [
    {
      id: "w-1",
      owner: RICH_ID,
      name: "alpha",
      state: "running",
      tokens: 1200,
      title: "任务一",
      startedAt: now - 5_000,
    },
    {
      id: "w-2",
      owner: RICH_ID,
      name: "beta",
      state: "idle",
      tokens: 400,
      tasks: 2,
      lastActive: now - 180_000,
    },
  ]);
  await expect(page.getByTestId("haa-toggle")).toHaveCount(0);
  await expect(page.getByTestId("haa-count")).toContainText("1/2");
  await expect(page.getByTestId("haa-worker")).toHaveCount(2);
  await assertAlignedNoOverflow(page);

  // 3 个以上：有 haa-toggle，默认只显示在干活的行；出现 asking 时不自动展开。
  await emitWorkers(page, [
    {
      id: "w-1",
      owner: RICH_ID,
      name: "alpha",
      state: "running",
      model: "codex/gpt-6.2-sol:high",
      tokens: 1200,
      ctx: 22,
      title: "任务一",
      startedAt: now - 10_000,
    },
    { id: "w-2", owner: RICH_ID, name: "beta", state: "idle", tokens: 0, tasks: 1, lastActive: now - 120_000 },
    { id: "w-3", owner: RICH_ID, name: "gamma", state: "dormant", tokens: 0, tasks: 4, lastActive: now - 7_200_000 },
  ]);
  const toggle = page.getByTestId("haa-toggle");
  await expect(toggle).toHaveAttribute("aria-expanded", "false");
  await expect(page.getByTestId("haa-worker")).toHaveCount(1);

  // 出现 asking 时仍保持收起，只显示在干活的 2 行，不会自动展开显示全部 3 行。
  await emitWorkers(page, [
    {
      id: "w-1",
      owner: RICH_ID,
      name: "alpha",
      state: "asking",
      model: "agy/gemini-4-neon-high",
      tokens: 2000,
      ctx: 51,
      title: "任务一",
      pending: "第一行问题\n第二行细节",
      startedAt: now - 20_000,
    },
    {
      id: "w-2",
      owner: RICH_ID,
      name: "beta",
      state: "running",
      model: "agent-gateway/deepseek-v4.1-flash",
      tokens: 500,
      ctx: 12,
      title: "一段很长很长的任务标题用于确认单行截断不会造成横向溢出",
      startedAt: now - 5_000,
    },
    { id: "w-3", owner: RICH_ID, name: "gamma", state: "idle", tokens: 0, tasks: 3, lastActive: now - 600_000 },
  ]);
  await expect(toggle).toHaveAttribute("aria-expanded", "false");
  await expect(page.getByTestId("haa-worker")).toHaveCount(2);

  const askingRow = page.locator('[data-testid="haa-worker"][data-state="asking"]');
  await expect(askingRow).toContainText("等待中");
  await expect(askingRow).toContainText("问 Main：第一行问题 第二行细节");

  // 点 haa-toggle 展开显示全部 3 行，行高一致且无横向溢出。
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-expanded", "true");
  await expect(page.getByTestId("haa-worker")).toHaveCount(3);
  await assertAlignedNoOverflow(page);

  // 点回收起，并切到 3 个全部空闲 / 休眠：只有一行，haa-summary 为「2 个空闲 · 1 个休眠」，haa-worker 为 0 个；点开后为 3 个。
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-expanded", "false");
  await emitWorkers(page, [
    { id: "w-1", owner: RICH_ID, name: "alpha", state: "idle", tokens: 100, tasks: 2, lastActive: now - 120_000 },
    { id: "w-2", owner: RICH_ID, name: "beta", state: "idle", tokens: 200, tasks: 1, lastActive: now - 300_000 },
    { id: "w-3", owner: RICH_ID, name: "gamma", state: "dormant", tokens: 300, tasks: 5, lastActive: now - 7_200_000 },
  ]);
  await expect(page.getByTestId("haa-summary")).toHaveText("2 个空闲 · 1 个休眠");
  await expect(page.getByTestId("haa-worker")).toHaveCount(0);
  await assertAlignedNoOverflow(page);

  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-expanded", "true");
  await expect(page.getByTestId("haa-worker")).toHaveCount(3);
  await assertAlignedNoOverflow(page);

  // 切到非 Live Session：停掉 watch 并清空状态条与 `--connect-h`。
  await open(page, REDIS_TITLE);
  await expect.poll(() => haaProbe(page)).toMatchObject({ watching: false, watchStarts: 1 });
  await expect(page.getByTestId("haa-workers")).toHaveCount(0);
  await expect.poll(() => connectHeightVar(page)).toBe("");
});

test("展开后 Connect、Disconnect 都保持展开，切换 Session 回到收起", async ({ page }) => {
  const workers: HaaWorker[] = [
    { id: "w-1", owner: RICH_ID, name: "alpha", state: "running", tokens: 500, title: "任务一" },
    { id: "w-2", owner: RICH_ID, name: "beta", state: "idle", tokens: 0, tasks: 1 },
    { id: "w-3", owner: RICH_ID, name: "gamma", state: "dormant", tokens: 0, tasks: 2 },
  ];
  await page.goto("/");
  await open(page, RICH_TITLE);
  await expect.poll(() => haaProbe(page)).toMatchObject({ watching: true, watchStarts: 1 });
  await emitWorkers(page, workers);
  const toggle = page.getByTestId("haa-toggle");
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-expanded", "true");
  await expect(page.getByTestId("haa-worker")).toHaveCount(3);

  // Connect 把状态条挪进 Connect 面板，Disconnect 再挪出来：都不收起。
  await pick(page, "connect-button");
  await expect(page.getByTestId("connect-panel").getByTestId("haa-workers")).toBeVisible();
  await expect(toggle).toHaveAttribute("aria-expanded", "true");
  await expect(page.getByTestId("haa-worker")).toHaveCount(3);
  await pick(page, "connect-disconnect");
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  await expect(toggle).toHaveAttribute("aria-expanded", "true");
  await expect(page.getByTestId("haa-worker")).toHaveCount(3);

  // 切到别的 Session 再回来：回到收起，只列在干活的。
  await open(page, REDIS_TITLE);
  await open(page, RICH_TITLE);
  await expect.poll(() => haaProbe(page)).toMatchObject({ watching: true, watchStarts: 2 });
  await emitWorkers(page, workers);
  await expect(toggle).toHaveAttribute("aria-expanded", "false");
  await expect(page.getByTestId("haa-worker")).toHaveCount(1);
});

test("已 Connect：状态条在请求与输入框之上，焦点在状态条上不触发 Connect 快捷键", async ({ page }) => {
  await page.goto("/");
  await open(page, RICH_TITLE);
  await page.getByTestId("header-more").click();
  await page.getByTestId("connect-button").click();
  await expect(page.getByTestId("connect-panel")).toBeVisible();
  await expect(page.getByTestId("composer")).toBeVisible();
  await expect.poll(() => haaProbe(page)).toMatchObject({ watching: true });

  await emitWorkers(page, [
    { id: "w-1", owner: RICH_ID, name: "alpha", state: "running", tokens: 500, title: "任务一" },
    { id: "w-2", owner: RICH_ID, name: "beta", state: "idle", tokens: 0, tasks: 1 },
    { id: "w-3", owner: RICH_ID, name: "gamma", state: "dormant", tokens: 0, tasks: 2 },
  ]);
  await page.evaluate(() => {
    window.__cvCcrpc!.emit({ type: "state", state: "waiting", waiting: "permission" });
    window.__cvCcrpc!.emit({
      type: "permission_request",
      request_id: "req-haa-order",
      tool_use_id: null,
      tool_name: "Bash",
      tool_input: { command: "echo ok" },
      description: null,
      agent: null,
      options: [
        { label: "允许", meaning: "allow" },
        { label: "总是允许", meaning: "allow_always" },
        { label: "拒绝", meaning: "deny" },
      ],
      layout_source: null,
    });
  });

  const bar = page.getByTestId("haa-workers");
  const toggle = page.getByTestId("haa-toggle");
  const reqs = page.getByTestId("connect-requests");
  const card = page.getByTestId("permission-card");
  const composer = page.getByTestId("composer");
  await expect(toggle).toBeVisible();
  await expect(card).toBeVisible();

  const barBox = (await bar.boundingBox())!;
  const reqsBox = (await reqs.boundingBox())!;
  const compBox = (await composer.boundingBox())!;
  expect(barBox.y + barBox.height).toBeLessThanOrEqual(reqsBox.y + 1);
  expect(reqsBox.y + reqsBox.height).toBeLessThanOrEqual(compBox.y + 1);

  // 焦点在状态条上按 1、2、Escape 不触发权限作答或中止；焦点离开后正常触发。
  const before = await page.evaluate(() => window.__cvCcrpc!.sent.length);
  await toggle.focus();
  await expect(toggle).toBeFocused();
  await page.keyboard.press("1");
  await page.keyboard.press("2");
  await page.keyboard.press("Escape");
  await nextFrames(page);
  expect(await page.evaluate((from) => window.__cvCcrpc!.sent.slice(from).map((s) => s.msg.type), before)).toEqual([]);
  await expect(card).toBeVisible();

  await card.getByTestId("allow").focus();
  await page.keyboard.press("1");
  await expect
    .poll(() =>
      page.evaluate(
        (from) =>
          window
            .__cvCcrpc!.sent.slice(from)
            .filter((s) => s.msg.type === "answer_permission")
            .map((s) => s.msg.decision),
        before,
      ),
    )
    .toEqual(["allow"]);
});

test("Ended 之后状态条消失且不再启动", async ({ page }) => {
  await page.goto("/");
  await open(page, RICH_TITLE);
  await expect.poll(() => haaProbe(page)).toMatchObject({ watching: true, watchStarts: 1 });
  await emitWorkers(page, [{ id: "w-1", owner: RICH_ID, name: "alpha", state: "running", tokens: 100 }]);
  await expect(page.getByTestId("haa-workers")).toBeVisible();

  await endWatch(page, "旧版不支持");
  await expect(page.getByTestId("haa-workers")).toHaveCount(0);
  await expect.poll(() => haaProbe(page)).toMatchObject({ watching: false, watchStarts: 1 });

  await open(page, REDIS_TITLE);
  await open(page, RICH_TITLE);
  await waitReady(page);
  expect(await haaProbe(page)).toMatchObject({ watching: false, watchStarts: 1 });
});
