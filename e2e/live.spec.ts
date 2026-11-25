import { expect, test } from "@playwright/test";
import { disableAutoConnect } from "./connect-helpers";
import { openRich, openSession, RICH_TITLE } from "./helpers";

// Exercise the index’s Live state with 自动 Connect off (see connect-lifecycle.spec.ts for 自动 Connect).
test.beforeEach(({ page }) => disableAutoConnect(page));

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
      body: {
        kind: "assistant",
        messageId: "m-e2e",
        model: null,
        usage: null,
        isApiError: false,
        blocks: [{ kind: "text", text: "E2E 追加的新消息" }],
      },
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
      body: {
        kind: "assistant",
        messageId: "m-e2e2",
        model: null,
        usage: null,
        isApiError: false,
        blocks: [{ kind: "text", text: "另一条新消息" }],
      },
    });
    window.__cvMock!.emit("sessionsChanged", { changed: [sid], removed: [], projectsChanged: false });
  }, id);
  await expect(page.getByTestId("new-messages")).toBeVisible();
  // Not placed by a transform: a half-pixel translate blurs its text.
  expect(
    await page.getByTestId("new-messages").evaluate((el) => {
      const cs = getComputedStyle(el);
      return [cs.transform, cs.translate];
    }),
  ).toEqual(["none", "none"]);
  await page.getByTestId("new-messages").click();
  await expect(page.getByText("另一条新消息")).toBeVisible();
});

test("打开的 Session 转入后台后切到接替它的 Session，列表里只剩后者；接替者再转入后台时接着切过去", async ({
  page,
}) => {
  const old = await openSession(page, "排查 Redis 连接池泄漏", { folded: true });
  const next = await page
    .getByTestId("session-row")
    .filter({ hasText: "迁移表单校验到 zod" })
    .getAttribute("data-session-id");

  await page.evaluate(([o, n]) => window.__cvMock!.supersede(o, n), [old, next!]);

  const header = page.getByTestId("header");
  await expect(header.getByRole("heading", { name: "迁移表单校验到 zod" })).toBeVisible();
  await expect(page.locator(`[data-testid="session-row"][data-session-id="${old}"]`)).toHaveCount(0);
  await expect(page.locator(`[data-testid="session-row"][data-session-id="${next}"]`)).toHaveAttribute(
    "aria-selected",
    "true",
  );

  // Moved to the background again while open: on to the newest Session.
  const third = await page
    .getByTestId("session-row")
    .filter({ hasText: "优化首页首屏加载" })
    .getAttribute("data-session-id");
  await page.evaluate(([o, n]) => window.__cvMock!.supersede(o, n), [next!, third!]);
  await expect(header.getByRole("heading", { name: /优化首页首屏加载/ })).toBeVisible();
});

test("转入后台的 Session 又单独显示后，打开它不再转到之前接替它的 Session", async ({ page }) => {
  const old = await openSession(page, "排查 Redis 连接池泄漏", { folded: true });
  const next = await page
    .getByTestId("session-row")
    .filter({ hasText: "迁移表单校验到 zod" })
    .getAttribute("data-session-id");
  const header = page.getByTestId("header");
  await page.evaluate(([o, n]) => window.__cvMock!.supersede(o, n), [old, next!]);
  await expect(header.getByRole("heading", { name: "迁移表单校验到 zod" })).toBeVisible();

  // Shown again (e.g. it went on by itself): its cached detail still names the old successor until refetched.
  await page.evaluate((o) => window.__cvMock!.supersede(o, null), old);
  await page.locator(`[data-testid="session-row"][data-session-id="${old}"]`).click();
  await expect(header.getByRole("heading", { name: "排查 Redis 连接池泄漏" })).toBeVisible();
  await page.waitForTimeout(300);
  await expect(header.getByRole("heading", { name: "排查 Redis 连接池泄漏" })).toBeVisible();
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
  await page.evaluate(
    (sid) => window.__cvMock!.setLive(sid, { status: "idle", rawStatus: "idle", waitingFor: null, pid: 4242 }),
    id,
  );
  await expect(dot).toHaveAttribute("data-state", "idle");
  await expect(page.getByTestId("live-state")).toHaveText("空闲");
  await page.evaluate((sid) => window.__cvMock!.setLive(sid, null), id);
  await expect(dot).toHaveCount(0);
  await expect(page.getByTestId("live-state")).toContainText("已结束");
});
