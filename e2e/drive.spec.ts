import { expect, test, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";

/** The `turn_end` prompt ids of a recorded fixture, in order (they change when it is re-recorded). */
function recordedTurns(name: string): string[] {
  return readFileSync(`src/mocks/data/drive/${name}.jsonl`, "utf8")
    .split("\n")
    .filter((l) => l.trim())
    .map((l) => JSON.parse(l))
    .filter((l) => l.dir === "recv" && l.msg?.type === "turn_end")
    .map((l) => l.msg.prompt_id as string);
}
const toolsTurns = recordedTurns("tools");

// Driving against the mock ccrpc (src/ipc/mock-drive.ts): each src/mocks/data/drive/*.jsonl is a
// recorded wire.jsonl replayed as one `ccrpc ls` entry, standing for a Session of the mock data:
// ui-cases → the rich Live Session, tools → the Flux Session, session-changed → the auth Session
// (its `/clear` moves to the JWT one), no-plugin → the LCP Live Session (plugin not reported),
// turns → the zod Session, trust → the review Session, refused → the Longhorn Session (its ccrpc exits
// before `driveConnect` returns), clay-new → a Session not in the mock index (opened in clay, no prompt yet),
// resumed → the untitled Session (it ends right after connecting, as if `/exit` in the terminal).

const RICH = "为 /v1/chat 接口增加令牌桶限流中间件";
const TOOLS = "为 Flux 增加 kestrel-cli 镜像自动更新";
const AUTH = "重构认证模块：拆分 token 校验与会话存储";
const JWT = "认证模块重构（改用 JWT 轮换）";
const NO_PLUGIN = "优化首页首屏加载（LCP 3.2s → 1.8s）";
const NOT_LISTED = "排查 Redis 连接池泄漏";
const TURNS = "迁移表单校验到 zod";
const TRUST = "/review 订单列表页";
const REFUSED = "Longhorn 卷扩容与备份策略";

type Probe = { sent: { msg: Record<string, unknown> }[]; lists: number; guiLog: string[]; badge: number; analyzerSaves: number };
const probe = (page: Page) => page.evaluate(() => window.__cvDrive as unknown as Probe);
const lastSent = async (page: Page, type: string) => (await probe(page)).sent.map((s) => s.msg).filter((m) => m.type === type).pop();

async function open(page: Page, title: string) {
  await page.getByTestId("session-row").filter({ hasText: title }).click();
  await expect(page.getByTestId("header").getByRole("heading", { name: title })).toBeVisible();
}

async function drive(page: Page, title: string) {
  await page.goto("/");
  await open(page, title);
  await page.getByTestId("drive-connect").click();
  await expect(page.getByTestId("drive-panel")).toBeVisible();
}

async function send(page: Page, text: string, mod?: "Meta") {
  const input = page.getByTestId("drive-input");
  await input.fill(text);
  await input.press(mod ? `${mod}+Enter` : "Enter");
}

const state = (page: Page) => page.getByTestId("drive-state");

/** Pastes a 1×1 PNG into the input box. */
async function pasteImage(page: Page) {
  await page.getByTestId("drive-input").focus();
  await page.evaluate(() => {
    const png = Uint8Array.from(atob("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg=="), (c) => c.charCodeAt(0));
    const dt = new DataTransfer();
    dt.items.add(new File([png], "dot.png", { type: "image/png" }));
    const ev = new Event("paste", { bubbles: true, cancelable: true }) as Event & { clipboardData: DataTransfer };
    Object.defineProperty(ev, "clipboardData", { value: dt });
    document.querySelector("[data-testid=drive-input]")!.dispatchEvent(ev);
  });
}

test("标题栏：ccrpc ls 里的 Live Session 可驾驶，插件未报到时禁用，其他 Session 没有", async ({ page }) => {
  await page.goto("/");
  await open(page, RICH);
  await expect(page.getByTestId("drive-connect")).toBeEnabled();
  await expect(page.getByTestId("drive-panel")).toHaveCount(0);

  await open(page, NO_PLUGIN);
  await expect(page.getByTestId("drive-connect")).toBeDisabled();
  await expect(page.getByTestId("drive-connect")).toHaveAttribute("title", "ccrpc 插件未加载，重启 claude 后可驾驶");

  await open(page, NOT_LISTED);
  await expect(page.getByTestId("header-more")).toBeVisible();
  await expect(page.getByTestId("drive-connect")).toHaveCount(0);
});

test("审批的三个选项、带说明的拒绝、AskUserQuestion", async ({ page }) => {
  await drive(page, TOOLS);
  await expect(state(page)).toContainText("空闲");
  expect((await probe(page)).guiLog).toContain("connect m8ded9f28b056");
  // CoDeck's single-key shortcuts (j / k) stay out of the input box.
  await page.getByTestId("drive-input").pressSequentially("jk");
  await expect(page.getByTestId("drive-input")).toHaveValue("jk");

  await send(page, "MOCK-TOOL please");
  const card = page.getByTestId("permission-card");
  await expect(card).toContainText("touch mock-tool.txt");
  await expect(card.getByTestId("tool-call")).toHaveAttribute("data-tool", "Bash");
  await expect(card.getByTestId("allow-always")).toContainText("always allow access to");
  await expect(card.getByTestId("allow")).toHaveText("1. 允许");
  await expect(card.getByTestId("deny")).toBeVisible();
  // Waiting: Enter does not send, it moves focus to the card.
  await page.getByTestId("drive-input").fill("先别发");
  await page.getByTestId("drive-input").press("Enter");
  await expect(card.getByTestId("allow")).toBeFocused();
  expect(await lastSent(page, "prompt")).toMatchObject({ text: "MOCK-TOOL please" });
  await card.getByTestId("allow").click();
  await expect(card).toHaveCount(0);
  expect(await lastSent(page, "answer_permission")).toMatchObject({ decision: "allow", request_id: "toolu_mock0000" });
  await expect(page.getByTestId("turn-end")).toHaveAttribute("data-prompt-id", toolsTurns[0]);

  await send(page, "MOCK-TOOL again");
  await page.getByTestId("permission-card").getByTestId("deny-with-message").click();
  await page.getByTestId("deny-message").fill("not now");
  await page.getByTestId("deny-message").press("Enter");
  await expect(page.getByTestId("turn-end")).toHaveAttribute("data-prompt-id", toolsTurns[1]);
  expect(await lastSent(page, "answer_permission")).toMatchObject({ decision: "deny", message: "not now" });

  await send(page, "MOCK-ASK which");
  // A main-line question is answered on its transcript card; the drive panel only points there.
  await expect(page.getByTestId("question-hint")).toBeVisible();
  await expect(page.getByTestId("question-card")).toHaveCount(0);
  await page.evaluate(() => {
    const sid = "a7c63e92-0008-4000-8000-000000000008";
    const input = { questions: [{ question: "Which color?", header: "Color", multiSelect: false, options: [{ label: "Red", description: "warm" }, { label: "Blue", description: "cool" }] }] };
    window.__cvMock!.appendNode(sid, {
      id: "e2e-ask",
      timestampMs: Date.now(),
      hidden: false,
      inherited: false,
      body: {
        kind: "assistant",
        messageId: "m-e2e-ask",
        model: null,
        usage: null,
        isApiError: false,
        blocks: [{ kind: "toolCall", toolUseId: "toolu_mock0002", name: "AskUserQuestion", inputJson: JSON.stringify(input), inputTruncated: false, result: null, subagentId: null, workflowRunId: null, notificationNodeId: null }],
      },
    });
    window.__cvMock!.emit("sessionsChanged", { changed: [sid], removed: [], projectsChanged: false });
  });
  await page.getByTestId("question-jump").click();
  const q = page.locator("[data-testid=ask][data-answering]");
  await expect(q).toContainText("Which color?");
  await expect(q.getByTestId("ask-status")).toHaveText("等待回答");
  // One answer: an option or the written one, never both.
  const blue = q.getByTestId("question-option").filter({ hasText: "Blue" });
  const otherRow = q.getByTestId("question-other-row");
  await blue.click();
  await expect(blue).toHaveAttribute("aria-pressed", "true");
  await q.getByTestId("question-other").fill("Green");
  await expect(blue).toHaveAttribute("aria-pressed", "false");
  await expect(otherRow).toHaveAttribute("data-on", "true");
  await blue.click();
  await expect(otherRow).not.toHaveAttribute("data-on");
  await expect(q.getByTestId("question-other")).toHaveValue("Green"); // set aside, not lost
  await q.getByTestId("submit-answer").click();
  await expect(page.getByTestId("turn-end")).toHaveAttribute("data-prompt-id", toolsTurns[2]);
  expect(await lastSent(page, "answer_question")).toMatchObject({ answers: [{ options: ["Blue"] }] });
});

test("忙时 Enter 插话（和终端里一样进 Claude Code 的队列）；数字键作答；Dock 角标；AI 识别的选择；Esc 中止；ccrpc 退出后重连", async ({ page }) => {
  await drive(page, RICH);
  await expect(state(page)).toContainText("空闲");
  // The header says the state once, in one word, with 「断开」 where 「驾驶」 was; the other drive controls sit at
  // the foot of the input box, the send button at its right end.
  const header = page.getByTestId("header");
  await expect(header.getByTestId("live-state")).toHaveText("空闲");
  await expect(header.getByTestId("drive-disconnect")).toBeVisible();
  await expect(header.getByTestId("drive-controls")).toHaveCount(0);
  const box = (await page.getByTestId("drive-input").boundingBox())!;
  expect((await page.getByTestId("drive-controls").boundingBox())!.y).toBeGreaterThan(box.y + box.height - 4);
  const sendButton = (await page.getByRole("button", { name: "发送" }).boundingBox())!;
  expect(box.x + box.width - (sendButton.x + sendButton.width)).toBeLessThan(4);
  // Background tasks: the running one only; finished ones on request.
  await expect(page.getByTestId("tasks")).toHaveText("1 个后台任务");
  await page.getByTestId("tasks").click();
  const tasks = page.getByTestId("tasks-list").locator("li");
  await expect(tasks).toHaveCount(1);
  await expect(tasks).toContainText("pnpm test:e2e --watch");
  await page.getByTestId("tasks-done").click();
  await expect(tasks).toHaveCount(2);
  await expect(tasks.nth(1)).toContainText("为限流中间件补集成测试");
  await page.keyboard.press("Escape");
  // The LCP Live Session waits (CoDeck's Live data).
  await expect.poll(async () => (await probe(page)).badge).toBe(1);

  await send(page, "改一下 README");
  await expect(state(page)).toContainText("工作中");
  // Claude Code's progress line is under the last message, in the transcript.
  await expect(page.getByTestId("transcript").getByTestId("progress")).toContainText("Thinking");

  await expect(page.getByTestId("drive-input")).toHaveAttribute("placeholder", /^插话/);
  // An image goes along with a steer too.
  await pasteImage(page);
  await expect(page.getByTestId("images").locator("img")).toHaveCount(1);
  await send(page, "做完再跑一下测试");
  await expect
    .poll(() => lastSent(page, "steer"))
    .toMatchObject({ text: "做完再跑一下测试", images: ["/mock/Caches/dev.joy.codeck/images/1.png"] });
  await expect(page.getByTestId("images")).toHaveCount(0);
  await expect(page.getByTestId("drive-input")).toHaveValue("");
  // The TUI's own queue, as the terminal shows it; sent at once on request (ctrl+enter there).
  await expect(page.getByTestId("queue-item")).toHaveText(["做完再跑一下测试"]);
  await page.getByTestId("send-now").click();
  await expect.poll(async () => (await probe(page)).sent.filter((s) => s.msg.type === "send_now").length).toBe(1);
  await expect(page.getByTestId("queue")).toHaveCount(0);

  // ⌘Enter: queued, then sent at once.
  await send(page, "顺便把标题也改了", "Meta");
  await expect.poll(() => lastSent(page, "steer")).toMatchObject({ text: "顺便把标题也改了" });
  await expect.poll(async () => (await probe(page)).sent.filter((s) => s.msg.type === "send_now").length).toBe(2);
  expect(await lastSent(page, "follow_up")).toBeUndefined();

  const card = page.getByTestId("permission-card");
  await expect(card).toContainText("README.md");
  await expect(card).toContainText("新的一行");
  // A learned layout: the buttons carry the dialog's own words, not fixed ones.
  await expect(card.getByTestId("allow")).toHaveText("1. Yes");
  await expect(card.getByTestId("deny")).toHaveText("3. No");
  // This Session's card and the LCP Session, counted once each.
  await expect.poll(async () => (await probe(page)).badge).toBe(2);
  // With focus in the conversation, number keys do not answer.
  await page.getByTestId("transcript").click({ position: { x: 4, y: 4 } });
  await page.keyboard.press("1");
  await page.waitForTimeout(200);
  await expect(card).toBeVisible();
  expect(await lastSent(page, "answer_permission")).toBeUndefined();
  // Enter in the (empty) input box moves focus to the card; then number keys answer it.
  await page.getByTestId("drive-input").press("Enter");
  await expect(card.getByTestId("allow")).toBeFocused();
  await page.keyboard.press("1");
  await expect(card).toHaveCount(0);
  expect(await lastSent(page, "answer_permission")).toMatchObject({ decision: "allow" });
  await expect(page.getByTestId("turn-end")).toHaveAttribute("data-prompt-id", "P1");
  await expect.poll(async () => (await probe(page)).badge).toBe(1);

  await send(page, "按计划来");
  const choice = page.getByTestId("choice-card");
  await expect(choice.getByTestId("ai-badge")).toBeVisible();
  await choice.getByTestId("choice-option").filter({ hasText: "manually approve" }).click();
  await expect(choice).toHaveCount(0);
  await expect(page.getByTestId("turn-end")).toHaveAttribute("data-prompt-id", "P2");
  expect(await lastSent(page, "answer_choice")).toMatchObject({ option: "Yes, manually approve edits" });

  await send(page, "重写第一条");
  const screen = page.getByTestId("screen-card");
  await expect(screen).toContainText("Something new");
  await expect(screen).toContainText("Mac Client");
  await page.getByTestId("drive-input").press("Escape");
  expect(await lastSent(page, "abort")).toBeTruthy();
  await expect(page.getByTestId("proc-exit")).toContainText("退出码 1");
  await expect(page.getByTestId("drive-input")).toHaveCount(0);

  await page.getByTestId("reconnect").click();
  await expect(page.getByTestId("proc-exit")).toHaveCount(0);
  await expect(state(page)).toContainText("空闲");
  expect((await probe(page)).guiLog).toContain("reconnect mui0000000001");
});

test("Pane 里换了 Session（/clear）时选中跟着切换；复制排查信息；断开", async ({ page }) => {
  await drive(page, AUTH);
  await expect(state(page)).toContainText("空闲");
  await send(page, "/clear");
  await expect(page.getByTestId("header").getByRole("heading", { name: JWT })).toBeVisible();
  await expect(page.getByTestId("drive-panel")).toBeVisible();

  await page.getByTestId("header-more").click();
  await page.getByRole("menuitem", { name: "复制排查信息" }).click();
  const copied = await page.evaluate(() => window.__cvCalls!.filter((c) => c.method === "copyText").pop()?.args[0]);
  expect(copied).toMatch(/^holder_id: mclear0000001\nsession_id: c04e77a2-0003-4000-8000-000000000003\ncwd: \/work\/lumen\nlogs: \S+$/);

  await page.getByTestId("drive-disconnect").click();
  await expect(page.getByTestId("drive-panel")).toHaveCount(0);
  await expect(page.getByTestId("drive-controls")).toHaveCount(0);
});

test("新建 Session：选目录后选中它，进索引前显示占位且可以发送", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("new-session").click();
  await expect(page.getByTestId("transcript-new")).toHaveText("新 Session，发送第一条提示词后显示对话");
  await expect(page.getByTestId("header")).toContainText("新 Session");
  await expect(page.getByTestId("header")).toContainText("/tmp/mock-new");
  await expect(state(page)).toContainText("空闲");
  await expect(page.getByTestId("drive-input")).toBeEnabled();
  expect((await probe(page)).guiLog).toContain("new /tmp/mock-new");
});

test("clay 里刚打开、未进索引的 Session 以占位行列在最上方，可选中、驾驶，按 cwd 参与 Project 筛选", async ({ page }) => {
  const id = "c1a70000-0000-4000-8000-0000000000c1";
  await page.goto("/");
  const row = page.getByTestId("session-row").first();
  await expect(row).toHaveAttribute("data-session-id", id);
  await expect(row).toContainText("新 Session");
  await expect(row).toContainText("services");
  await expect(row.getByTestId("live-dot")).toHaveAttribute("data-state", "idle");

  await row.click();
  await expect(row).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("transcript-new")).toHaveText("新 Session，发送第一条提示词后显示对话");
  await expect(page.getByTestId("header")).toContainText("/Users/dev/Developer/lumen-api/services");
  await page.getByTestId("drive-connect").click();
  await expect(page.getByTestId("drive-panel")).toBeVisible();
  await expect(state(page)).toContainText("空闲");
  await expect(page.getByTestId("drive-input")).toBeEnabled();

  const filterBy = async (name: string) => {
    await page.keyboard.press("Meta+K");
    await page.getByTestId("search").getByRole("combobox").fill(name);
    await page.getByTestId("search-project").filter({ hasText: name }).click();
    await expect(page.getByTestId("project-chip")).toContainText(name);
  };
  const placeholder = page.locator(`[data-testid=session-row][data-session-id="${id}"]`);
  await filterBy("lumen-api");
  await expect(placeholder).toBeVisible();
  await page.getByRole("button", { name: "清除 Project 筛选" }).click();
  await filterBy("orbit-web");
  await expect(page.getByTestId("session-row").first()).toBeVisible();
  await expect(placeholder).toHaveCount(0);
});

test("斜杠命令：输入 / 列出补全，Tab 补全；内置命令走 slash，自定义命令走 prompt，打开的面板可关闭", async ({ page }) => {
  await page.goto("/");
  await page.locator('[data-testid=session-row][data-session-id="c1a70000-0000-4000-8000-0000000000c1"]').click();
  await page.getByTestId("drive-connect").click();
  await expect(state(page)).toContainText("空闲");
  const input = page.getByTestId("drive-input");
  const menu = page.getByTestId("slash-menu");
  const options = menu.getByTestId("slash-option");

  // `/` alone lists every command; the list scrolls with the pick.
  await input.fill("/");
  await expect(options).toHaveCount(10);
  await input.press("ArrowUp");
  await expect(options.last()).toHaveAttribute("aria-selected", "true");
  await expect(options.last()).toBeInViewport();

  await input.fill("/re");
  await expect(options).toHaveText([/^\/rename/, /^\/release-notes/, /^\/release/, /^\/kit:review/]);
  await expect(options.first()).toHaveAttribute("aria-selected", "true");
  await input.press("ArrowDown");
  await expect(options.nth(1)).toHaveAttribute("aria-selected", "true");
  await input.press("Tab");
  await expect(input).toHaveValue("/release-notes ");
  await expect(menu).toHaveCount(0);
  await input.press("Enter");
  await expect.poll(() => lastSent(page, "slash")).toMatchObject({ command: "release-notes" });
  await expect(input).toHaveValue("");

  // Esc only hides the list.
  await input.fill("/co");
  await expect(options.first()).toContainText("/compact");
  await expect(menu.getByTestId("slash-option").filter({ hasText: "/cost" })).toContainText("打开界面");
  await input.press("Escape");
  await expect(menu).toHaveCount(0);
  await expect(input).toHaveValue("/co");

  // A project command waits for its record in the transcript: `prompt`.
  await input.fill("/release v1");
  await input.press("Enter");
  await expect.poll(() => lastSent(page, "prompt")).toMatchObject({ text: "/release v1" });
  await expect(input).toHaveValue("");

  // `/cost` opens a panel ccrpc cannot read: shown as is, closed from the card.
  await expect(state(page)).toContainText("空闲");
  await input.fill("/cost");
  await input.press("Enter");
  await expect.poll(() => lastSent(page, "slash")).toMatchObject({ command: "cost" });
  const screen = page.getByTestId("screen-card");
  await expect(screen).toContainText("Total cost");
  await screen.getByTestId("screen-close").click();
  expect(await lastSent(page, "abort")).toBeTruthy();
  await expect(screen).toHaveCount(0);
  await expect(state(page)).toContainText("空闲");
});

test("设置 › 驾驶：ccrpc 路径、doctor 的失败项、日志目录；分析器的 Key、Endpoint、模型可在这里设置", async ({ page }) => {
  await page.goto("/");
  await page.keyboard.press("Meta+,");
  await page.getByRole("tab", { name: "驾驶" }).click();
  await expect(page.getByTestId("set-ccrpc")).toContainText("/mock/bin/ccrpc");
  await expect(page.getByTestId("set-ccrpc")).toContainText("CODECK_CCRPC");
  await expect(page.getByTestId("set-doctor").locator('[data-level="fail"]')).toContainText("ccrpc 插件未启用");
  await expect(page.getByTestId("set-drive-logs")).toContainText("/mock/Logs/dev.joy.codeck/drive");

  // The analyzer: a key from the login shell until one is saved here, with an endpoint and model.
  const status = page.getByTestId("analyzer-status");
  await expect(status).toHaveText("已开启 · Key 来自登录 shell 的 CCRPC_ANALYZER_API_KEY（…abcd）");
  await expect(page.getByTestId("analyzer-base-url")).toHaveAttribute("placeholder", "https://api.deepseek.com/anthropic");
  await page.getByTestId("analyzer-key").fill("sk-ant-test-9876");
  await page.getByTestId("analyzer-base-url").fill("https://api.anthropic.com");
  await page.getByTestId("analyzer-model").fill("claude-haiku-4-5");
  await page.getByTestId("analyzer-save").click();
  await expect(status).toHaveText("已开启 · Key 保存在这里（…9876）");
  await expect(page.getByTestId("analyzer-key")).toHaveValue("");
  await expect(page.getByTestId("analyzer-base-url")).toHaveValue("https://api.anthropic.com");
  await expect(page.getByTestId("set-analyzer")).toContainText("之后新建的驾驶连接生效");
  // Saving again with the key box empty keeps the key; clearing it goes back to the shell's.
  await page.getByTestId("analyzer-save").click();
  await expect(status).toHaveText("已开启 · Key 保存在这里（…9876）");
  await page.getByTestId("analyzer-clear-key").click();
  await expect(status).toContainText("来自登录 shell");
  expect((await probe(page)).analyzerSaves).toBe(3);
});

test("ccrpc ls 只在有 Live Session 或驾驶连接时轮询，恢复时立即查一次", async ({ page }) => {
  await page.goto("/");
  await open(page, RICH);
  await expect(page.getByTestId("drive-connect")).toBeVisible();
  const ids = await page.evaluate(() =>
    // Indexed rows only: the clay-new placeholder is Live through `ccrpc ls`, not the mock data.
    [...document.querySelectorAll("[data-testid=session-row]:not([data-pending]) [data-testid=live-dot]")].map((d) =>
      d.closest("[data-session-id]")!.getAttribute("data-session-id")!,
    ),
  );
  expect(ids.length).toBeGreaterThan(0);
  await page.evaluate((list) => list.forEach((id) => window.__cvMock!.setLive(id, null)), ids);
  await expect(page.getByTestId("live-dot")).toHaveCount(0);
  // The last `ccrpc ls` is dropped with nothing Live, and no more are run.
  await expect(page.getByTestId("drive-connect")).toHaveCount(0);
  const before = (await probe(page)).lists;
  await page.waitForTimeout(4500);
  expect((await probe(page)).lists).toBe(before);
  // 「新建 Session」 works without an `ccrpc ls` answer.
  await expect(page.getByTestId("new-session")).toBeVisible();

  await page.evaluate((id) => window.__cvMock!.setLive(id, { status: "idle", rawStatus: "idle", waitingFor: null, pid: 1, source: "processFile" }), ids[0]!);
  await expect.poll(async () => (await probe(page)).lists, { timeout: 1500 }).toBe(before + 1);
  await expect(page.getByTestId("drive-connect")).toBeVisible();
});

test("ccrpc 在连接命令返回前就退出：显示退出横幅而不是一直「连接中」", async ({ page }) => {
  await drive(page, REFUSED);
  await expect(page.getByTestId("proc-exit")).toContainText("退出码 1");
  await expect(page.getByTestId("proc-exit")).toContainText("正被 mac 使用");
  // Only the banner says it: no input box under it.
  await expect(page.getByTestId("drive-input")).toHaveCount(0);
  await expect(page.getByTestId("reconnect")).toBeVisible();
});

test("Session 在终端里退出后：驾驶区只剩一行提示，头部重新出现「驾驶」，resume 后可再次连接", async ({ page }) => {
  await drive(page, "（无标题）");
  await expect(page.getByTestId("session-exit")).toHaveText("Session 已结束");
  await expect(page.getByTestId("drive-input")).toHaveCount(0);
  await expect(page.getByTestId("drive-disconnect")).toHaveCount(0);
  // Still in `ccrpc ls` (resumed in the terminal): driving again replaces the ended connection.
  await page.getByTestId("drive-connect").click();
  await expect.poll(async () => (await probe(page)).guiLog.filter((l) => l === "connect mresumed00001").length).toBe(2);
  expect((await probe(page)).guiLog).toContain("close (replaced)");
  await expect(page.getByTestId("session-exit")).toHaveCount(1);
});

test("目录信任框是选择卡片", async ({ page }) => {
  await drive(page, TRUST);
  const card = page.getByTestId("choice-card");
  await expect(card).toContainText("trust");
  await expect(card.getByTestId("ai-badge")).toHaveCount(0);
  await card.getByTestId("choice-option").filter({ hasText: "Yes, I trust this folder" }).click();
  await expect(card).toHaveCount(0);
  await expect(state(page)).toContainText("空闲");
  expect(await lastSent(page, "answer_choice")).toMatchObject({ option: "Yes, I trust this folder" });
});

test("粘贴的图片随提示词发出", async ({ page }) => {
  await drive(page, TURNS);
  await expect(state(page)).toContainText("空闲");
  await pasteImage(page);
  const preview = page.getByTestId("images").locator("img");
  await expect(preview).toHaveCount(1);
  // The app's CSP allows data: images only.
  expect(await preview.getAttribute("src")).toMatch(/^data:image\/png;base64,/);
  await expect.poll(() => preview.evaluate((i: HTMLImageElement) => i.naturalWidth)).toBeGreaterThan(0);
  await send(page, "MOCK-SLOW please");
  expect((await lastSent(page, "prompt"))?.images).toEqual(["/mock/Caches/dev.joy.codeck/images/1.png"]);
  await expect(page.getByTestId("images")).toHaveCount(0);
});

test("没发出的输入跟着 Session，切换、重启后仍在，发出后清掉", async ({ page }) => {
  await drive(page, TURNS);
  await expect(state(page)).toContainText("空闲");
  await page.getByTestId("drive-input").fill("还没写完的草稿");
  await pasteImage(page);
  await expect(page.getByTestId("images").locator("img")).toHaveCount(1);
  await open(page, TOOLS);
  await page.getByTestId("drive-connect").click();
  await expect(state(page)).toContainText("空闲");
  await expect(page.getByTestId("drive-input")).toHaveValue("");
  await expect(page.getByTestId("images")).toHaveCount(0);
  await open(page, TURNS);
  await expect(page.getByTestId("drive-input")).toHaveValue("还没写完的草稿");
  await expect(page.getByTestId("images").locator("img")).toHaveCount(1);

  // And across a restart: back when the Session is driven again.
  await page.reload();
  await open(page, TURNS);
  await page.getByTestId("drive-connect").click();
  await expect(page.getByTestId("drive-input")).toHaveValue("还没写完的草稿");
  await expect(page.getByTestId("images").locator("img")).toHaveCount(1);
  // Sent, it is gone for good.
  await page.getByTestId("drive-input").press("Enter");
  await expect(page.getByTestId("drive-input")).toHaveValue("");
  await page.reload();
  await open(page, TURNS);
  await page.getByTestId("drive-connect").click();
  await expect(state(page)).toContainText("空闲");
  await expect(page.getByTestId("drive-input")).toHaveValue("");
});

test("切回窗口时当前 Session 无事而别处在等作答：顶部一行提示，点击切过去", async ({ page }) => {
  await page.goto("/");
  // The LCP Live Session waits: no hint while it is the one on screen.
  await open(page, NO_PLUGIN);
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await page.waitForTimeout(200);
  await expect(page.getByTestId("elsewhere")).toHaveCount(0);

  await open(page, NOT_LISTED);
  // With the list hidden the toolbar floats over the conversation's top strip: the line stays clear of it, under the header.
  await page.keyboard.press("Control+Meta+L");
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await expect(page.getByTestId("elsewhere")).toHaveText(`${NO_PLUGIN} 在等你作答，点此切过去`);
  const line = (await page.getByTestId("elsewhere").boundingBox())!;
  const tools = (await page.getByTestId("toolbar").boundingBox())!;
  const header = (await page.getByTestId("header").boundingBox())!;
  expect(line.y).toBeGreaterThanOrEqual(tools.y + tools.height);
  expect(line.y).toBeGreaterThanOrEqual(header.y + header.height - 1);
  await page.keyboard.press("Control+Meta+L");
  await page.getByTestId("elsewhere").click();
  await expect(page.getByTestId("header").getByRole("heading", { name: NO_PLUGIN })).toBeVisible();
  await expect(page.getByTestId("elsewhere")).toHaveCount(0);
});

test("Project 筛选不影响 Dock 角标：筛掉的 Project 里等待作答的 Session 仍计入", async ({ page }) => {
  await page.goto("/");
  // The LCP Live Session (orbit-web) waits.
  await expect.poll(async () => (await probe(page)).badge).toBe(1);
  await page.keyboard.press("Meta+K");
  await page.getByTestId("search").getByRole("combobox").fill("homelab");
  await page.getByTestId("search-project").filter({ hasText: "homelab" }).click();
  await expect(page.getByTestId("project-chip")).toContainText("homelab");
  await expect(page.getByTestId("session-row").filter({ hasText: NO_PLUGIN })).toHaveCount(0);
  await page.waitForTimeout(500);
  expect((await probe(page)).badge).toBe(1);
});
