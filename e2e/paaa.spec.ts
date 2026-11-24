import { expect, test, type Page } from "@playwright/test";
import type { PaaaWorker } from "../src/features/paaa/protocol";
import { RICH_TITLE } from "./helpers";

const REDIS_TITLE = "排查 Redis 连接池泄漏";
const REDIS_ID = "91d5be38-0004-4000-8000-000000000004";
const RICH_ID = "7f3a9c10-0001-4000-8000-000000000001";

const paaaProbe = (page: Page) =>
  page.evaluate(() => {
    const { watching, watchStarts, ready } = window.__cvPaaa!;
    return { watching, watchStarts, ready };
  });

const emitWorkers = (page: Page, workers: PaaaWorker[]) =>
  page.evaluate((list) => window.__cvPaaa!.emit(list), workers);

const endWatch = (page: Page, message: string) => page.evaluate((msg) => window.__cvPaaa!.end(msg), message);

const nextFrames = (page: Page) =>
  page.evaluate(() => new Promise<void>((r) => requestAnimationFrame(() => requestAnimationFrame(() => r()))));

async function waitReady(page: Page) {
  await expect(page.getByTestId("transcript").locator("[data-row]").first()).toBeVisible();
  await expect.poll(() => paaaProbe(page).then((p) => p.ready)).toBe(true);
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

test("历史（非 Live）Session 不启动 watch", async ({ page }) => {
  await page.goto("/");
  await open(page, REDIS_TITLE);
  await waitReady(page);
  expect(await paaaProbe(page)).toMatchObject({ watching: false, watchStarts: 0, ready: true });
  await expect(page.getByTestId("paaa-workers")).toHaveCount(0);
});

test("Live 未 Connect：启动 watch、过滤 owner、等回答自动展开、切到非 Live Session 停掉并清空", async ({ page }) => {
  await page.goto("/");
  await open(page, RICH_TITLE);
  await expect.poll(() => paaaProbe(page)).toMatchObject({ watching: true, watchStarts: 1 });
  await expect(page.getByTestId("paaa-workers")).toHaveCount(0);

  // 仅推入其他 Session 的 paaa Worker 时不显示状态条。
  await emitWorkers(page, [{ id: "w-other", owner: REDIS_ID, name: "stranger", state: "running", tokens: 999 }]);
  await nextFrames(page);
  await expect(page.getByTestId("paaa-workers")).toHaveCount(0);

  // 推入本 Session 与其他 Session 的 paaa Worker：只显示 owner 匹配的，并撑开 `--connect-h`。
  await emitWorkers(page, [
    {
      id: "w-own-1",
      owner: RICH_ID,
      name: "alpha",
      state: "running",
      tokens: 18400,
      ctx: 37.4,
      title: "修复限流边界",
      activity: "cargo test",
      startedAt: Date.now() - 15_000,
    },
    { id: "w-other", owner: REDIS_ID, name: "stranger", state: "running", tokens: 999 },
  ]);

  const bar = page.getByTestId("paaa-workers");
  await expect(bar).toBeVisible();
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  await expect(bar).toContainText("alpha");
  await expect(bar).toContainText("运行中");
  await expect(bar).toContainText("18.4K token");
  await expect(bar).toContainText("37%");
  await expect(bar).toContainText("修复限流边界");
  await expect(bar).toContainText("cargo test");
  await expect(bar).not.toContainText("stranger");
  await expect.poll(() => connectHeightVar(page)).not.toBe("");

  // 多个 paaa Worker 默认折叠；等回答时自动展开并展示可换行、可键盘聚焦的问题。
  await emitWorkers(page, [
    { id: "w-1", owner: RICH_ID, name: "alpha", state: "running", tokens: 1200, title: "任务一" },
    { id: "w-2", owner: RICH_ID, name: "beta", state: "idle", tokens: 0, title: "任务二" },
  ]);
  const toggle = page.getByTestId("paaa-toggle");
  await expect(toggle).toContainText("1 运行中");
  await expect(toggle).toContainText("1 空闲");
  await expect(page.getByTestId("paaa-worker")).toHaveCount(0);

  await emitWorkers(page, [
    {
      id: "w-1",
      owner: RICH_ID,
      name: "alpha",
      state: "asking",
      tokens: 2000,
      title: "任务一",
      pending: "第一行问题\n第二行细节",
    },
    { id: "w-2", owner: RICH_ID, name: "beta", state: "running", tokens: 500, title: "任务二" },
  ]);
  await expect(page.getByTestId("paaa-worker")).toHaveCount(2);
  const pending = page.getByTestId("paaa-pending");
  await expect(pending).toHaveText("第一行问题\n第二行细节");
  await expect(pending).toHaveCSS("white-space", "pre-wrap");
  await expect(pending).toHaveAttribute("tabindex", "0");

  // 切到非 Live Session：停掉 watch 并清空状态条与 `--connect-h`。
  await open(page, REDIS_TITLE);
  await expect.poll(() => paaaProbe(page)).toMatchObject({ watching: false, watchStarts: 1 });
  await expect(page.getByTestId("paaa-workers")).toHaveCount(0);
  await expect.poll(() => connectHeightVar(page)).toBe("");
});

test("已 Connect：状态条在请求与输入框之上，焦点在状态条上不触发 Connect 快捷键", async ({ page }) => {
  await page.goto("/");
  await open(page, RICH_TITLE);
  await page.getByTestId("header-more").click();
  await page.getByTestId("connect-button").click();
  await expect(page.getByTestId("connect-panel")).toBeVisible();
  await expect(page.getByTestId("composer")).toBeVisible();
  await expect.poll(() => paaaProbe(page)).toMatchObject({ watching: true });

  await emitWorkers(page, [
    { id: "w-1", owner: RICH_ID, name: "alpha", state: "running", tokens: 500, title: "任务一" },
    { id: "w-2", owner: RICH_ID, name: "beta", state: "idle", tokens: 0, title: "任务二" },
  ]);
  await page.evaluate(() => {
    window.__cvCcrpc!.emit({ type: "state", state: "waiting", waiting: "permission" });
    window.__cvCcrpc!.emit({
      type: "permission_request",
      request_id: "req-paaa-order",
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

  const bar = page.getByTestId("paaa-workers");
  const toggle = page.getByTestId("paaa-toggle");
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
  await expect.poll(() => paaaProbe(page)).toMatchObject({ watching: true, watchStarts: 1 });
  await emitWorkers(page, [{ id: "w-1", owner: RICH_ID, name: "alpha", state: "running", tokens: 100 }]);
  await expect(page.getByTestId("paaa-workers")).toBeVisible();

  await endWatch(page, "旧版不支持");
  await expect(page.getByTestId("paaa-workers")).toHaveCount(0);
  await expect.poll(() => paaaProbe(page)).toMatchObject({ watching: false, watchStarts: 1 });

  await open(page, REDIS_TITLE);
  await open(page, RICH_TITLE);
  await waitReady(page);
  expect(await paaaProbe(page)).toMatchObject({ watching: false, watchStarts: 1 });
});
