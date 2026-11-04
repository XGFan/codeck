import { expect, test, type Page } from "@playwright/test";

// Against `vite --mode mock`: each e2e/fixtures/*.jsonl is a recorded wire.jsonl replayed as one
// Session (src/ipc/mock.ts). tools / session / turns are recorded from ccrpc's mock-model end-to-end
// tests (scripts/record-fixtures.sh); ui-cases, waiting-elsewhere and new-session are hand-written.

type Sent = { conn: string; msg: Record<string, unknown> };
const sent = (page: Page) => page.evaluate(() => (window as unknown as { __mock: { sent: Sent[] } }).__mock.sent.map((s) => s.msg));
const lastSent = async (page: Page, type: string) => (await sent(page)).filter((m) => m.type === type).pop();

async function open(page: Page, title: string) {
  await page.goto("/");
  await page.getByTestId("session-row").filter({ hasText: title }).click();
  await expect(page.getByTestId("header")).toBeVisible();
}

async function send(page: Page, text: string, mod?: "Meta") {
  const input = page.getByTestId("input");
  await input.fill(text);
  await input.press(mod ? `${mod}+Enter` : "Enter");
}

/** Pastes a 1×1 PNG into the input box. */
async function pasteImage(page: Page) {
  await page.getByTestId("input").focus();
  await page.evaluate(async () => {
    const png = Uint8Array.from(atob("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg=="), (c) => c.charCodeAt(0));
    const dt = new DataTransfer();
    dt.items.add(new File([png], "dot.png", { type: "image/png" }));
    const ev = new Event("paste", { bubbles: true, cancelable: true }) as Event & { clipboardData: DataTransfer };
    Object.defineProperty(ev, "clipboardData", { value: dt });
    document.querySelector("[data-testid=input]")!.dispatchEvent(ev);
  });
}

test("lists Sessions, the waiting one first, and counts it on the Dock badge", async ({ page }) => {
  await page.goto("/");
  const rows = page.getByTestId("session-row");
  await expect(rows.first()).toContainText("另一个项目");
  await expect(rows.first()).toHaveAttribute("data-waiting", "true");
  await expect(rows).toHaveCount(5);
  await expect.poll(() => page.evaluate(() => (window as unknown as { __mock: { badge: number } }).__mock.badge)).toBe(1);
});

test("approval with the dialog's options, denial with feedback, AskUserQuestion", async ({ page }) => {
  await open(page, "tools");
  await expect(page.getByTestId("state")).toHaveText("空闲");

  await send(page, "MOCK-TOOL please");
  const card = page.getByTestId("permission-card");
  await expect(card).toContainText("touch mock-tool.txt");
  // "Always allow" shows what the dialog itself says it adds.
  await expect(card.getByTestId("allow-always")).toContainText("always allow access to");
  await card.getByTestId("allow").click();
  await expect(card).toHaveCount(0);
  expect(await lastSent(page, "answer_permission")).toMatchObject({ decision: "allow", request_id: "toolu_mock0000" });
  await expect(page.getByTestId("turn-end")).toHaveCount(1);
  await expect(page.getByTestId("tool-call").first()).toHaveAttribute("data-tool", "Bash");

  await send(page, "MOCK-TOOL again");
  await page.getByTestId("permission-card").getByTestId("deny-with-message").click();
  await page.getByTestId("deny-message").fill("not now");
  await page.getByTestId("deny-message").press("Enter");
  await expect(page.getByTestId("turn-end")).toHaveCount(2);
  expect(await lastSent(page, "answer_permission")).toMatchObject({ decision: "deny", message: "not now" });

  await send(page, "MOCK-ASK which");
  const q = page.getByTestId("question-card");
  await expect(q).toContainText("Which color?");
  await q.getByTestId("question-option").filter({ hasText: "Blue" }).click();
  await q.getByTestId("submit-answer").click();
  await expect(page.getByTestId("turn-end")).toHaveCount(3);
  expect(await lastSent(page, "answer_question")).toMatchObject({ answers: [{ options: ["Blue"] }] });
});

test("the folder trust dialog is a choice card", async ({ page }) => {
  await open(page, "session");
  const card = page.getByTestId("choice-card");
  await expect(card).toContainText("trust");
  await expect(card.getByTestId("ai-badge")).toHaveCount(0);
  await card.getByTestId("choice-option").filter({ hasText: "Yes, I trust this folder" }).click();
  await expect(card).toHaveCount(0);
  await expect(page.getByTestId("state")).toHaveText("空闲");
});

test("busy: Enter queues a follow-up (cancellable), ⌘Enter steers; Edit approval, Subagent panel", async ({ page }) => {
  await open(page, "界面用例");
  await send(page, "改一下 README");
  await expect(page.getByTestId("state")).toHaveText("忙");
  await expect(page.getByTestId("progress")).toContainText("Thinking");

  await send(page, "做完再跑一下测试");
  await expect(page.getByTestId("follow-ups")).toContainText("做完再跑一下测试");
  expect(await lastSent(page, "follow_up")).toMatchObject({ text: "做完再跑一下测试" });

  await send(page, "顺便把标题也改了", "Meta");
  expect(await lastSent(page, "steer")).toMatchObject({ text: "顺便把标题也改了" });
  await expect(page.getByTestId("transcript")).toContainText("顺便把标题也改了");

  const card = page.getByTestId("permission-card");
  await expect(card).toContainText("README.md");
  await expect(card).toContainText("新的一行");
  await page.getByTestId("cancel-follow-up").click();
  await expect(page.getByTestId("follow-ups")).toHaveCount(0);
  // Number keys answer the first card.
  await page.getByTestId("transcript").click();
  await page.keyboard.press("1");
  await expect(card).toHaveCount(0);
  expect(await lastSent(page, "answer_permission")).toMatchObject({ decision: "allow" });

  await expect(page.getByTestId("turn-end")).toHaveCount(1);
  await page.getByTestId("open-subagent").first().click();
  const panel = page.getByTestId("subagent-panel");
  await expect(panel).toContainText("在 docs/ 下找到 2 处引用");
});

test("a learned choice is marked, a branch drops what it replaced, an unknown dialog shows the screen, ccrpc exit offers reconnect", async ({ page }) => {
  await open(page, "界面用例");
  // Through the first turn quickly.
  await send(page, "改一下 README");
  await send(page, "做完再跑一下测试");
  await send(page, "顺便把标题也改了", "Meta");
  await page.getByTestId("permission-card").getByTestId("allow").click();
  await expect(page.getByTestId("turn-end")).toHaveCount(1);
  // The prompt's image, fetched from ccrpc.
  await expect(page.getByTestId("prompt-image")).toHaveCount(1);
  await expect.poll(() => page.getByTestId("prompt-image").evaluate((i: HTMLImageElement) => i.naturalWidth)).toBeGreaterThan(0);

  await send(page, "按计划来");
  const choice = page.getByTestId("choice-card");
  await expect(choice.getByTestId("ai-badge")).toBeVisible();
  await choice.getByTestId("choice-option").filter({ hasText: "manually approve" }).click();
  await expect(page.getByTestId("turn-end")).toHaveCount(2);
  await expect(page.getByTestId("transcript")).toContainText("开始执行计划");

  await send(page, "重写第一条");
  const t = page.getByTestId("transcript");
  await expect(t).toContainText("重写第一条");
  await expect(t).not.toContainText("开始执行计划");
  await expect(t).toContainText("改一下 README");

  const screen = page.getByTestId("screen-card");
  await expect(screen).toContainText("Something new");
  await expect(screen).toContainText("Mac Client");
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("proc-exit")).toContainText("退出码 1");
  await expect(page.getByTestId("reconnect")).toBeVisible();
  await expect(page.getByTestId("input")).toBeDisabled();
});

test("a new Session is started in the picked folder", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("new-session").click();
  await expect(page.getByTestId("header")).toContainText("/tmp/mock-new");
  await expect(page.getByTestId("state")).toHaveText("空闲");
});

test("a pasted image goes with the prompt", async ({ page }) => {
  await open(page, "turns");
  await pasteImage(page);
  const preview = page.getByTestId("images").locator("img");
  await expect(preview).toHaveCount(1);
  // The app's CSP allows data: images only (a blob: preview showed blank).
  expect(await preview.getAttribute("src")).toMatch(/^data:image\/png;base64,/);
  await expect.poll(() => preview.evaluate((i: HTMLImageElement) => i.naturalWidth)).toBeGreaterThan(0);
  await send(page, "MOCK-SLOW please");
  const p = await lastSent(page, "prompt");
  expect(p?.images).toEqual(["/tmp/cc-gui-mock-1.png"]);
  await expect(page.getByTestId("images")).toHaveCount(0);
});

test("unsent input stays with its Session across switches", async ({ page }) => {
  await open(page, "turns");
  await page.getByTestId("input").fill("还没写完的草稿");
  await pasteImage(page);
  await expect(page.getByTestId("images").locator("img")).toHaveCount(1);
  const row = (title: string) => page.getByTestId("session-row").filter({ hasText: title });
  await row("tools").click();
  await expect(page.getByTestId("state")).toHaveText("空闲");
  await expect(page.getByTestId("input")).toHaveValue("");
  await expect(page.getByTestId("images")).toHaveCount(0);
  await row("turns").click();
  await expect(page.getByTestId("input")).toHaveValue("还没写完的草稿");
  await expect(page.getByTestId("images").locator("img")).toHaveCount(1);
});

test("the header copies what is needed to look into a Session", async ({ page }) => {
  await open(page, "turns");
  const header = page.getByTestId("header");
  await header.hover();
  await header.getByRole("button", { name: "复制排查信息" }).click();
  await expect(header).toContainText("已复制");
  const copied = await page.evaluate(() => (window as unknown as { __mock: { copied: string[] } }).__mock.copied.pop());
  expect(copied).toMatch(/^holder_id: \S+\nsession_id: \S+\ncwd: \S+\nlogs: \/mock\/logs\/mock\d+$/);
});

test("a long stream scrolls inside its area, following the newest turn", async ({ page }) => {
  await page.setViewportSize({ width: 900, height: 420 });
  await open(page, "界面用例");
  await send(page, "改一下 README");
  await send(page, "做完再跑一下测试");
  await send(page, "顺便把标题也改了", "Meta");
  await page.getByTestId("permission-card").getByTestId("allow").click();
  await expect(page.getByTestId("turn-end")).toHaveCount(1);
  const t = page.getByTestId("transcript");
  const box = await t.boundingBox();
  const composer = await page.getByTestId("composer").boundingBox();
  // It stays above the input box and scrolls rather than growing past it.
  expect(box!.y + box!.height).toBeLessThanOrEqual(composer!.y + 1);
  const m = await t.evaluate((el) => ({ sh: el.scrollHeight, ch: el.clientHeight, top: el.scrollTop }));
  expect(m.sh).toBeGreaterThan(m.ch);
  await expect.poll(() => t.evaluate((el) => el.scrollHeight - el.scrollTop - el.clientHeight)).toBeLessThan(40);
});
