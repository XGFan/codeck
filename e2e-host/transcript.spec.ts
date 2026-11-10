import { appendFileSync, readFileSync } from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { SESSION_FILE, SESSION_ID } from "./global-setup";

const page_url = `/web/transcript.html?sessionId=${SESSION_ID}`;

test("对话页渲染真实后端里的 Session，不带 Connect", async ({ page }) => {
  await page.goto(page_url);
  await expect(page.getByTestId("transcript-page")).toBeVisible();
  await expect(page.getByText("Add retry with exponential backoff to the HTTP client").first()).toBeVisible({
    timeout: 20_000,
  });
  await expect(page.getByTestId("composer")).toHaveCount(0);
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
});

test("App 写入的内边距生效：顶部让出，底部变高时仍贴底，滚离底部时告诉 App", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 320 });
  await page.addInitScript(() => {
    const w = window as unknown as { posted: unknown[]; webkit: unknown };
    w.posted = [];
    w.webkit = { messageHandlers: { codeck: { postMessage: (m: unknown) => w.posted.push(m) } } };
  });
  await page.goto(page_url);
  await expect(page.getByText("Add retry with exponential backoff to the HTTP client").first()).toBeVisible({
    timeout: 20_000,
  });
  const scroller = page.locator("[data-scope]").first();
  const gap = () => scroller.evaluate((el) => el.scrollHeight - el.scrollTop - el.clientHeight);
  const setVar = (name: string, value: string) =>
    page.evaluate(([n, v]) => document.documentElement.style.setProperty(n!, v!), [name, value]);

  await setVar("--transcript-top", "70px");
  await expect(scroller).toHaveCSS("padding-top", "70px");

  await scroller.evaluate((el) => {
    el.scrollTop = el.scrollHeight;
  });
  // The list learns it is at the end from the scroll event, dispatched with the next frame.
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
  await setVar("--connect-h", "400px");
  await expect.poll(gap).toBeLessThanOrEqual(2);

  await scroller.evaluate((el) => {
    el.scrollTop = 0;
  });
  await expect
    .poll(() => page.evaluate(() => (window as unknown as { posted: unknown[] }).posted))
    .toContainEqual({ type: "scroll", atBottom: false });
});

test("/v1/rpc 同源：list_sessions 返回 fixture，local 方法被拒", async ({ page }) => {
  await page.goto(page_url);
  const out = await page.evaluate(async () => {
    const call = async (method: string, args: unknown) =>
      (
        await fetch("/v1/rpc", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ method, args }),
        })
      ).json();
    const query = { query: { projectIds: [], sort: "lastActive", descending: true } };
    return { list: await call("list_sessions", query), local: await call("set_data_root", { path: null }) };
  });
  expect(out.list.result.some((s: { id: string }) => s.id === SESSION_ID)).toBe(true);
  expect(out.local.error.code).toBe("invalidQuery");
});

test("Origin: null 被拒（403）", async ({ request }) => {
  const res = await request.get("/health", { headers: { Origin: "null" } });
  expect(res.status()).toBe(403);
});

test("写入新的一行后页面随之更新", async ({ page }) => {
  await page.goto(page_url);
  await expect(page.getByText("Shipped.").first()).toBeVisible({ timeout: 20_000 });

  const file = path.join(process.env.E2E_HOST_DATA_ROOT!, SESSION_FILE);
  const leaf = "0bd99c42-5d43-5421-bf13-548ddfe11016";
  const line = {
    parentUuid: leaf,
    isSidechain: false,
    promptId: "11111111-2222-4333-8444-555555555555",
    type: "user",
    message: { role: "user", content: "live follow marker 7e1c" },
    uuid: "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
    timestamp: "2026-06-02T09:00:00.000Z",
    permissionMode: "default",
    origin: { kind: "human" },
    userType: "external",
    entrypoint: "cli",
    cwd: "/Users/dev/code/lumen-api",
    sessionId: SESSION_ID,
    version: "2.1.207",
    gitBranch: "main",
  };
  const prev = readFileSync(file, "utf8");
  appendFileSync(file, (prev.endsWith("\n") ? "" : "\n") + JSON.stringify(line) + "\n");

  await expect(page.getByText("live follow marker 7e1c").first()).toBeVisible({ timeout: 10_000 });
});
