import { expect, type Locator, test } from "@playwright/test";
import { reveal } from "./helpers";
import { unfoldAll } from "./unfold";

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

const LONGHORN_ID = "48b1d0f6-0009-4000-8000-000000000009";

test.describe("手指点按", () => {
  test.use({ hasTouch: true, viewport: { width: 393, height: 852 } });

  test("点你消息里的图片块直接放大，不闪一下悬停预览", async ({ page }) => {
    await page.goto(`/transcript.html?sessionId=${LONGHORN_ID}`);
    const chip = page.getByTestId("user-bubble").filter({ hasText: "PVC" }).getByTestId("prompt-image");
    await chip.scrollIntoViewIfNeeded();
    await chip.tap();
    await expect(page.getByTestId("image-large")).toBeVisible();
    await expect(page.getByTestId("prompt-image-preview")).toHaveCount(0);
  });

  const opacity = (l: Locator) => l.evaluate((e) => getComputedStyle(e).opacity);

  test("复制按钮与工具行的展开箭头不用悬停就显示", async ({ page }) => {
    await page.goto(`/transcript.html?sessionId=${RICH_ID}`);
    const t = page.getByTestId("transcript");
    const meta = page.getByTestId("prompt-meta").last();
    await expect(meta).toBeVisible();
    expect(await opacity(meta.getByTestId("copy-button"))).toBe("1");
    const bash = t.locator('[data-tool="Bash"]').filter({ hasText: "cargo test --workspace" });
    await unfoldAll(page);
    await reveal(t, bash, "top");
    const row = bash.getByRole("button").first();
    expect(await opacity(row.locator("svg").last())).toBe("1");
    await row.tap();
    expect(await opacity(bash.getByRole("button", { name: "复制命令" }))).toBe("1");
    // Always shown, it sits above a code block's text instead of over its first line.
    const block = t.locator("pre:not([data-testid])").filter({ hasText: "令牌按秒均匀补充" });
    await reveal(t, block);
    const room = await block.evaluate((pre) => {
      const button = pre.parentElement!.querySelector("[data-testid=copy-button]")!.getBoundingClientRect();
      return pre.getBoundingClientRect().top + parseFloat(getComputedStyle(pre).paddingTop) - button.bottom;
    });
    expect(room).toBeGreaterThanOrEqual(0);
  });

  test("提问卡片里点一下带预览的选项，换成它的预览", async ({ page }) => {
    await page.goto(`/transcript.html?sessionId=${RICH_ID}`);
    const t = page.getByTestId("transcript");
    const ask = page.getByTestId("ask").filter({ hasText: "限流超限时应返回什么？" });
    await unfoldAll(page);
    await reveal(t, ask);
    const layer = ask.getByTestId("ask-question").nth(2);
    const preview = layer.getByTestId("option-preview");
    await expect(preview).toHaveAttribute("data-option", "0");
    await layer.getByText("最外层").tap();
    await expect(preview).toHaveAttribute("data-option", "1");
  });

  test("手指碰到对话记录的右边缘不会翻一屏", async ({ page }) => {
    await page.goto(`/transcript.html?sessionId=${RICH_ID}`);
    const t = page.getByTestId("transcript");
    await expect(t.locator("[data-row]").first()).toBeVisible();
    await t.evaluate((e) => (e.scrollTop = e.scrollHeight / 2));
    await page.waitForTimeout(300);
    const before = await t.evaluate((e) => e.scrollTop);
    // Off the scrollbar's thumb (halfway down, as the list is): where a click would page up.
    const box = (await t.boundingBox())!;
    await page.touchscreen.tap(box.x + box.width - 3, box.y + 80);
    await page.waitForTimeout(300);
    expect(await t.evaluate((e) => e.scrollTop)).toBe(before);
  });
});

test("在 App 里，Connect 工作时对话记录末尾显示进度，和桌面一样", async ({ page }) => {
  await page.addInitScript(() => {
    Object.assign(window, { webkit: { messageHandlers: { codeck: { postMessage: () => {} } } } });
  });
  await page.goto(`/transcript.html?sessionId=${RICH_ID}`);
  await expect(page.getByTestId("prompt-meta").first()).toBeVisible();
  const progress = page.getByTestId("transcript").getByTestId("progress");
  await expect(progress).toHaveCount(0);
  const setConnect = (c: Record<string, unknown>) =>
    page.evaluate(
      (c) => window.codeckPage!.setConnect(c),
      c as Parameters<NonNullable<Window["codeckPage"]>["setConnect"]>[0],
    );
  await setConnect({ connected: true, busy: true, progress: "运行 pnpm test" });
  await expect(progress).toHaveText("运行 pnpm test");
  // At the end of the conversation, below its last row.
  const below = await progress.evaluate((p) => {
    const rows = [...document.querySelectorAll("[data-row]")];
    return p.getBoundingClientRect().top >= rows.at(-1)!.getBoundingClientRect().bottom - 1;
  });
  expect(below).toBe(true);
  await setConnect({ connected: true, busy: true });
  await expect(progress).toHaveText("工作中…");
  await setConnect({ connected: true, idle: true });
  await expect(progress).toHaveCount(0);
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
  expect((await posted()).filter((m) => m.type !== "ready")).toEqual([
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
