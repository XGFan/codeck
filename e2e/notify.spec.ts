import { expect, test, type Page } from "@playwright/test";
import { openApp, RICH_TITLE } from "./helpers";

const WAITING_TITLE = "优化首页首屏加载（LCP 3.2s → 1.8s）";

type Probe = {
  notifications: {
    id: string;
    title: string;
    body: string;
    url: string;
    thread: string | null;
    timeSensitive: boolean;
  }[];
  clickNotification(url: string): void;
  keepAlive: number;
  badge: number;
  keepAliveEvent(sessionId: string, active: boolean): void;
  removedNotifications: string[];
};
const notifications = (page: Page) =>
  page.evaluate(() => (window.__cvCcrpc as unknown as Probe).notifications.map((n) => ({ ...n })));

const liveRow = (page: Page, title: string) => page.getByTestId("live-row").filter({ hasText: title });

/** The busy mock Session ends its turn (its process file says idle). */
async function endTurn(page: Page) {
  const id = await page.getByTestId("live-row").filter({ hasText: RICH_TITLE }).getAttribute("data-session-id");
  await page.evaluate(
    (sid) => window.__cvMock!.setLive(sid!, { status: "idle", rawStatus: "idle", waitingFor: null, pid: 48211 }),
    id,
  );
  return id!;
}

/** Notification delays as saved in 「设置 › 通用」 (ms; null = off), before the app starts. */
const delays = (page: Page, d: { waiting: number | null; done: number | null }) =>
  page.addInitScript((d) => localStorage.setItem("codeck.notify", JSON.stringify(d)), d);

test("一轮结束时没在看它：侧栏标「已完成」，打开即清除", async ({ page }) => {
  await page.goto("/");
  const row = liveRow(page, RICH_TITLE);
  await expect(row.getByTestId("live-dot")).toHaveAttribute("data-state", "busy");
  await endTurn(page);
  await expect(row.getByTestId("done-tag")).toHaveText("已完成");
  await expect(row.getByTestId("live-dot")).toHaveAttribute("data-state", "done");

  await row.click();
  await expect(page.getByTestId("header").getByRole("heading", { name: RICH_TITLE })).toBeVisible();
  await expect(page.getByTestId("done-tag")).toHaveCount(0);
  await expect(row.getByTestId("live-dot")).toHaveAttribute("data-state", "idle");
});

test("看着它时一轮结束不标「已完成」", async ({ page }) => {
  await page.goto("/");
  await liveRow(page, RICH_TITLE).click();
  await expect(page.getByTestId("header").getByRole("heading", { name: RICH_TITLE })).toBeVisible();
  await endTurn(page);
  await expect(liveRow(page, RICH_TITLE).getByTestId("live-dot")).toHaveAttribute("data-state", "idle");
  await expect(page.getByTestId("done-tag")).toHaveCount(0);
});

test("等待作答到设定时长才发通知，点通知打开该 Session", async ({ page }) => {
  await page.clock.install();
  await page.goto("/");
  await expect(liveRow(page, WAITING_TITLE)).toBeVisible();
  await page.clock.fastForward("02:50");
  expect(await notifications(page)).toEqual([]);
  await page.clock.fastForward("00:15");
  await expect.poll(() => notifications(page)).toHaveLength(1);
  const [n] = await notifications(page);
  expect(n.title).toBe(WAITING_TITLE);
  expect(n.body).toMatch(/^等待作答 3 分钟 · /);
  // Its own in Notification Center: named and grouped by the Session, and let through a Focus.
  const sessionId = await liveRow(page, WAITING_TITLE).getAttribute("data-session-id");
  expect(n).toMatchObject({ id: `session-${sessionId}`, thread: sessionId, timeSensitive: true });
  // Once only.
  await page.clock.fastForward("05:00");
  expect(await notifications(page)).toHaveLength(1);

  await page.evaluate((url) => (window.__cvCcrpc as unknown as Probe).clickNotification(url), n.url);
  await expect(page.getByTestId("header").getByRole("heading", { name: WAITING_TITLE })).toBeVisible();
  // Looked at: it leaves Notification Center too.
  await page.clock.fastForward("00:02");
  await expect
    .poll(() => page.evaluate(() => (window.__cvCcrpc as unknown as Probe).removedNotifications))
    .toEqual([n.id]);
});

test("已完成到点发通知", async ({ page }) => {
  await delays(page, { waiting: 0, done: 0 });
  await page.goto("/");
  await expect.poll(() => notifications(page)).toHaveLength(1);
  await endTurn(page);
  await expect.poll(() => notifications(page)).toHaveLength(2);
  expect((await notifications(page))[1]).toMatchObject({
    title: RICH_TITLE,
    body: expect.stringMatching(/^已完成/),
    timeSensitive: false,
  });
});

test("设置里关闭的通知不发", async ({ page }) => {
  await delays(page, { waiting: 0, done: 0 });
  await openApp(page);
  await page.keyboard.press("Meta+,");
  await page.getByTestId("notify-waiting-off").click();
  await page.getByTestId("notify-done-off").click();
  await expect(page.getByTestId("notify-done-off")).toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("Escape");
  const before = (await notifications(page)).length;
  await endTurn(page);
  await expect(liveRow(page, RICH_TITLE).getByTestId("done-tag")).toBeVisible();
  await page.waitForTimeout(1500);
  expect(await notifications(page)).toHaveLength(before);
  // Kept for the next start.
  expect(await page.evaluate(() => JSON.parse(localStorage.getItem("codeck.notify")!))).toEqual({
    waiting: null,
    done: null,
  });
});

test("缓存保活期间 /btw 面板的等待不算等待作答，「已完成」也不清掉", async ({ page }) => {
  await page.goto("/");
  const id = await endTurn(page);
  const row = liveRow(page, RICH_TITLE);
  await expect(row.getByTestId("done-tag")).toBeVisible();
  const badge = () => page.evaluate(() => (window.__cvCcrpc as unknown as Probe).badge);
  const before = await badge();
  // The Host's `/btw` panel: the process file says waiting for a moment.
  await page.evaluate((sid) => (window.__cvCcrpc as unknown as Probe).keepAliveEvent(sid, true), id);
  await page.evaluate(
    (sid) =>
      window.__cvMock!.setLive(sid, { status: "waiting", rawStatus: "waiting", waitingFor: "dialog open", pid: 48211 }),
    id,
  );
  await page.waitForTimeout(500);
  await expect(row.getByTestId("waiting-tag")).toHaveCount(0);
  expect(await badge()).toBe(before);
  await page.evaluate(
    (sid) => window.__cvMock!.setLive(sid, { status: "idle", rawStatus: "idle", waitingFor: null, pid: 48211 }),
    id,
  );
  await page.evaluate((sid) => (window.__cvCcrpc as unknown as Probe).keepAliveEvent(sid, false), id);
  await expect(row.getByTestId("done-tag")).toBeVisible();
});

test("设置里的缓存保活：默认 40 分钟，可关闭", async ({ page }) => {
  await openApp(page);
  await page.keyboard.press("Meta+,");
  await expect(page.getByTestId("keep-alive-40")).toHaveAttribute("aria-checked", "true");
  await page.getByTestId("keep-alive-0").click();
  await expect(page.getByTestId("keep-alive-0")).toHaveAttribute("aria-checked", "true");
  expect(await page.evaluate(() => (window.__cvCcrpc as unknown as Probe).keepAlive)).toBe(0);
});
