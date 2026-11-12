import { expect, test } from "@playwright/test";

const RICH_ID = "7f3a9c10-0001-4000-8000-000000000001";

test("iPhone 用的对话页只渲染对话，不带 Connect 与侧栏", async ({ page }) => {
  await page.goto(`/transcript.html?sessionId=${RICH_ID}`);
  await expect(page.getByTestId("transcript-page")).toBeVisible();
  await expect(page.getByTestId("prompt-meta").first()).toBeVisible();
  await expect(page.getByTestId("pane-sessions")).toHaveCount(0);
  await expect(page.getByTestId("header")).toHaveCount(0);
  await expect(page.getByTestId("composer")).toHaveCount(0);
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  await expect(page.getByTestId("fork-button").first()).toBeHidden();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
});

// In the app (a WKWebView with the `codeck` message handler): the ended Redis Session, two Main Line prompts.
const REDIS_ID = "91d5be38-0004-4000-8000-000000000004";

test("在 App 里，提示词下的「回退」「Fork」按 App 给的状态显示，点了交给 App", async ({ page }) => {
  await page.addInitScript(() => {
    const posted: unknown[] = [];
    Object.assign(window, {
      __posted: posted,
      webkit: { messageHandlers: { codeck: { postMessage: (m: unknown) => posted.push(m) } } },
    });
  });
  const setConnect = (c: Record<string, unknown>) =>
    page.evaluate(
      (c) => window.codeckPage!.setConnect(c),
      c as Parameters<NonNullable<Window["codeckPage"]>["setConnect"]>[0],
    );
  const posted = () => page.evaluate(() => (window as unknown as { __posted: { type: string }[] }).__posted);
  await page.goto(`/transcript.html?sessionId=${REDIS_ID}`);
  const metas = page.getByTestId("prompt-meta");
  await expect(metas).toHaveCount(2);
  // Until the app says so: no Connect, no Fork.
  await expect(page.getByTestId("rewind-button")).toHaveCount(0);
  await expect(page.getByTestId("fork-button")).toHaveCount(0);

  // Not connected: Fork from the second prompt on, shown without hovering.
  await setConnect({ canFork: true });
  await expect(metas.nth(0).getByTestId("fork-button")).toHaveCount(0);
  await expect(metas.nth(1).getByTestId("fork-button")).toBeVisible();
  await expect(page.getByTestId("rewind-button")).toHaveCount(0);
  await metas.nth(1).getByTestId("fork-button").click();
  await expect(page.getByTestId("fork-menu")).toHaveCount(0);

  // Connected and idle: 「回退」 under both.
  await setConnect({ canFork: true, connected: true, idle: true });
  await expect(page.getByTestId("rewind-button")).toHaveCount(2);
  await metas.nth(1).getByTestId("rewind-button").click();
  expect((await posted()).filter((m) => m.type !== "ready" && m.type !== "scroll")).toEqual([
    { type: "fork", nodeId: "91d5-u1" },
    { type: "rewind", nodeId: "91d5-u1" },
  ]);

  // The confirm page is being opened, then the Session is busy: no second rewind.
  await setConnect({ canFork: true, connected: true, idle: true, rewinding: "91d5-u1" });
  await expect(metas.nth(1).getByTestId("rewind-button")).toHaveText("回退中…");
  await expect(metas.nth(0).getByTestId("rewind-button")).toBeDisabled();
  await setConnect({ canFork: true, connected: true, idle: false, forking: true });
  await expect(metas.nth(0).getByTestId("rewind-button")).toBeDisabled();
  await expect(metas.nth(1).getByTestId("fork-button")).toBeDisabled();

  // Rewound to before the second prompt: marked from there on, and only the first can still be rewound to.
  await setConnect({ canFork: true, connected: true, idle: true, rewound: { uuid: "91d5-u1", withCode: false } });
  await expect(page.getByTestId("rewound-divider")).toContainText("已回退到这里");
  await expect(page.getByTestId("rewind-button")).toHaveCount(1);
  await expect(metas.nth(0).getByTestId("rewind-button")).toBeEnabled();
});
