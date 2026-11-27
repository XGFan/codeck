import { expect, test, type Page } from "@playwright/test";
import { openApp, RICH_TITLE } from "./helpers";

const WAITING_TITLE = "优化首页首屏加载（LCP 3.2s → 1.8s）";

type Probe = {
  clickNotification(url: string): void;
  viewing: (string | null)[];
  notify: {
    waitingMinutes: number | null;
    doneMinutes: number | null;
    escalateMinutes: number | null;
    barkServer: string;
    barkDeviceKey: string;
  };
  lastPushError: { message: string; at: number } | null;
  barkTests: { server: string; deviceKey: string }[];
  barkTestError: string | null;
  keepAlive: number;
  badge: number;
  keepAliveEvent(sessionId: string, active: boolean): void;
};
const probe = <T>(page: Page, f: (p: Probe) => T) =>
  page.evaluate((src) => new Function("p", `return (${src})(p)`)(window.__cvCcrpc), f.toString()) as Promise<T>;

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

test("一轮结束时没在看它：侧栏标「已完成」；打开它即告诉 Host 在看它，标记清除", async ({ page }) => {
  await page.goto("/");
  const row = liveRow(page, RICH_TITLE);
  await expect(row.getByTestId("live-dot")).toHaveAttribute("data-state", "busy");
  const id = await endTurn(page);
  await expect(row.getByTestId("done-tag")).toHaveText("已完成");
  await expect(row.getByTestId("live-dot")).toHaveAttribute("data-state", "done");

  await row.click();
  await expect(page.getByTestId("header").getByRole("heading", { name: RICH_TITLE })).toBeVisible();
  await expect.poll(() => probe(page, (p) => p.viewing.at(-1))).toBe(id);
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

test("点通知打开它说的 Session", async ({ page }) => {
  await page.goto("/");
  const id = await liveRow(page, WAITING_TITLE).getAttribute("data-session-id");
  await page.evaluate((url) => (window.__cvCcrpc as unknown as Probe).clickNotification(url), `codeck://session/${id}`);
  await expect(page.getByTestId("header").getByRole("heading", { name: WAITING_TITLE })).toBeVisible();
  await expect.poll(() => probe(page, (p) => p.viewing.at(-1))).toBe(id);
});

test("通知的时长存在 Host 的设置里，补推默认 5 分钟", async ({ page }) => {
  await openApp(page);
  await page.keyboard.press("Meta+,");
  await expect(page.getByTestId("notify-waiting-3")).toHaveAttribute("aria-checked", "true");
  await expect(page.getByTestId("notify-escalate-5")).toHaveAttribute("aria-checked", "true");
  await page.getByTestId("notify-waiting-off").click();
  await page.getByTestId("notify-done-0").click();
  await page.getByTestId("notify-escalate-off").click();
  await expect(page.getByTestId("notify-escalate-off")).toHaveAttribute("aria-checked", "true");
  expect(await probe(page, (p) => p.notify)).toMatchObject({
    waitingMinutes: null,
    doneMinutes: 0,
    escalateMinutes: null,
  });
});

test("Bark：测试推送用框里的值，保存后推送；最近一次失败写在下面", async ({ page }) => {
  await openApp(page);
  await page.evaluate(() => {
    (window.__cvCcrpc as unknown as Probe).lastPushError = { message: "Bark 返回 400：bad key", at: Date.now() };
  });
  await page.keyboard.press("Meta+,");
  await expect(page.getByTestId("bark-server")).toHaveValue("https://api.day.app");
  await expect(page.getByTestId("bark-test")).toBeDisabled();
  await expect(page.getByTestId("bark-last-error")).toContainText("最近一次推送失败：Bark 返回 400：bad key");

  await page.getByTestId("bark-key").fill("K1");
  await page.getByTestId("bark-test").click();
  await expect(page.getByTestId("bark-test-result")).toHaveText("已发送");
  expect(await probe(page, (p) => p.barkTests)).toEqual([{ server: "https://api.day.app", deviceKey: "K1" }]);
  // Not saved by the test.
  expect(await probe(page, (p) => p.notify.barkDeviceKey)).toBe("");

  await page.evaluate(() => {
    (window.__cvCcrpc as unknown as Probe).barkTestError = "连不上 Bark 服务器";
  });
  await page.getByTestId("bark-test").click();
  await expect(page.getByTestId("bark-test-result")).toHaveText("发送失败：连不上 Bark 服务器");

  await page.getByTestId("bark-save").click();
  await expect(page.getByTestId("bark-save")).toBeDisabled();
  expect(await probe(page, (p) => p.notify.barkDeviceKey)).toBe("K1");
});

test("旧版存在本机的通知时长搬进 Host 的设置", async ({ page }) => {
  await page.addInitScript(() => {
    if (!sessionStorage.getItem("seeded")) {
      localStorage.setItem("codeck.notify", JSON.stringify({ waiting: 0, done: null }));
      sessionStorage.setItem("seeded", "1");
    }
  });
  await openApp(page);
  await expect.poll(() => probe(page, (p) => [p.notify.waitingMinutes, p.notify.doneMinutes])).toEqual([0, null]);
  expect(await page.evaluate(() => localStorage.getItem("codeck.notify"))).toBeNull();
});

test("缓存保活期间 /btw 面板的等待不算等待作答，「已完成」也不清掉", async ({ page }) => {
  await page.goto("/");
  const id = await endTurn(page);
  const row = liveRow(page, RICH_TITLE);
  await expect(row.getByTestId("done-tag")).toBeVisible();
  const badge = () => probe(page, (p) => p.badge);
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
  expect(await probe(page, (p) => p.keepAlive)).toBe(0);
});
