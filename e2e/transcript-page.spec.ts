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

test("手机宽度下，你的消息里放不下的长路径换行，气泡不超出页面", async ({ page }) => {
  await page.setViewportSize({ width: 393, height: 852 });
  await page.goto(`/transcript.html?sessionId=${RICH_ID}`);
  const bubble = page.getByTestId("user-bubble").filter({ hasText: "接下来把限流参数" });
  await bubble.scrollIntoViewIfNeeded();
  await expect(bubble).toContainText("rate_limiting.md");
  const r = await bubble.evaluate((el) => {
    const b = el.getBoundingClientRect();
    return { left: b.left, right: b.right, width: window.innerWidth };
  });
  expect(r.left).toBeGreaterThanOrEqual(0);
  expect(r.right).toBeLessThanOrEqual(r.width);
});

const CODE_MODE_ID = "c0de5e55-0011-4000-8000-000000000011";

test("在 App 里，Code Mode 代码运行期间的调用按 App 给的 codeCalls 并进对话记录", async ({ page }) => {
  await page.addInitScript(() => {
    Object.assign(window, { webkit: { messageHandlers: { codeck: { postMessage: () => {} } } } });
  });
  await page.goto(`/transcript.html?sessionId=${CODE_MODE_ID}`);
  const group = page.getByTestId("tool-group").last();
  await expect(group).toContainText("执行 2 段代码");
  await expect(page.getByTestId("group-current")).toHaveCount(0);
  const call = {
    toolUseId: "repl_e2e_bench",
    name: "Bash",
    input: { command: "cargo bench --bench retry" },
    code: "cm-r4",
  };
  await page.evaluate((c) => window.codeckPage!.setConnect({ codeCalls: [{ ...c, end: null }] } as never), call);
  await expect(page.getByTestId("group-current")).toContainText("正在运行 cargo bench --bench retry…");
  await page.evaluate(
    (c) =>
      window.codeckPage!.setConnect({
        codeCalls: [{ ...c, end: { isError: false, response: { stdout: "retry/backoff  1.21 ms", stderr: "" } } }],
      } as never),
    call,
  );
  await expect(page.getByTestId("group-current")).toHaveCount(0);
  await expect(group).toContainText("运行 1 条命令、执行 1 段代码");
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
