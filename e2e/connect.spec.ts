import { expect, test, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";

/** The `turn_end` prompt ids of a recorded fixture, in order (they change when it is re-recorded). */
function recordedTurns(name: string): string[] {
  return readFileSync(`src/mocks/data/connect/${name}.jsonl`, "utf8")
    .split("\n")
    .filter((l) => l.trim())
    .map((l) => JSON.parse(l))
    .filter((l) => l.dir === "recv" && l.msg?.type === "turn_end")
    .map((l) => l.msg.prompt_id as string);
}
const toolsTurns = recordedTurns("tools");

// Connect against the mock ccrpc (src/ipc/mock-ccrpc.ts): each src/mocks/data/connect/*.jsonl is a
// recorded wire.jsonl replayed as one `ccrpc ls` entry, standing for a Session of the mock data:
// ui-cases → the rich Live Session, tools → the Flux Session, session-changed → the auth Session
// (its `/clear` moves to the JWT one), no-plugin → the LCP Live Session (plugin not reported),
// turns → the zod Session, trust → the review Session, refused → the Longhorn Session (its ccrpc exits
// before `ccrpcConnect` returns), clay-new → a Session not in the mock index (opened in clay, no prompt yet),
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

type Probe = {
  sent: { msg: Record<string, unknown> }[];
  watchStarts: number;
  watching: boolean;
  guiLog: string[];
  badge: number;
  analyzerSaves: number;
  paneHides: { holderId: string; hidden: boolean }[];
  hideNewPanes: boolean;
};
const probe = (page: Page) => page.evaluate(() => window.__cvCcrpc as unknown as Probe);
const lastSent = async (page: Page, type: string) =>
  (await probe(page)).sent
    .map((s) => s.msg)
    .filter((m) => m.type === type)
    .pop();

async function open(page: Page, title: string) {
  await page.getByTestId("session-row").filter({ hasText: title }).click();
  await expect(page.getByTestId("header").getByRole("heading", { name: title })).toBeVisible();
}

async function connectTo(page: Page, title: string) {
  await page.goto("/");
  await open(page, title);
  await page.getByTestId("connect-button").click();
  await expect(page.getByTestId("connect-panel")).toBeVisible();
}

async function send(page: Page, text: string, mod?: "Meta") {
  const input = page.getByTestId("connect-input");
  await input.fill(text);
  await input.press(mod ? `${mod}+Enter` : "Enter");
}

const state = (page: Page) => page.getByTestId("connect-state");

/** Pastes a 1×1 PNG into the input box. */
async function pasteImage(page: Page) {
  await page.getByTestId("connect-input").focus();
  await page.evaluate(() => {
    const png = Uint8Array.from(
      atob("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg=="),
      (c) => c.charCodeAt(0),
    );
    const dt = new DataTransfer();
    dt.items.add(new File([png], "dot.png", { type: "image/png" }));
    const ev = new Event("paste", { bubbles: true, cancelable: true }) as Event & { clipboardData: DataTransfer };
    Object.defineProperty(ev, "clipboardData", { value: dt });
    document.querySelector("[data-testid=connect-input]")!.dispatchEvent(ev);
  });
}

test("标题栏：ccrpc ls 里的 Live Session 可 Connect，插件未报到时禁用，其他 Session 没有", async ({ page }) => {
  await page.goto("/");
  await open(page, RICH);
  await expect(page.getByTestId("connect-button")).toBeEnabled();
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);

  await open(page, NO_PLUGIN);
  await expect(page.getByTestId("connect-button")).toBeDisabled();
  await expect(page.getByTestId("connect-button")).toHaveAttribute(
    "title",
    "ccrpc 插件未加载，重启 claude 后可 Connect",
  );

  await open(page, NOT_LISTED);
  await expect(page.getByTestId("header-more")).toBeVisible();
  await expect(page.getByTestId("connect-button")).toHaveCount(0);
});

test("标题栏：clay 里的 Session 不论是否 Connect 都可隐藏 / 显示 Pane，不在 clay 里的没有", async ({ page }) => {
  await page.goto("/");
  await open(page, RICH);
  const btn = page.getByTestId("pane-visibility");
  await expect(btn).toHaveText("隐藏 Pane");
  await btn.click();
  await expect(btn).toHaveText("显示 Pane");
  const [hide] = (await probe(page)).paneHides;
  expect(hide.hidden).toBe(true);

  await page.getByTestId("connect-button").click();
  await expect(page.getByTestId("connect-panel")).toBeVisible();
  await expect(btn).toHaveText("显示 Pane");
  await btn.click();
  await expect(btn).toHaveText("隐藏 Pane");
  expect((await probe(page)).paneHides).toEqual([hide, { holderId: hide.holderId, hidden: false }]);

  await open(page, NOT_LISTED);
  await expect(page.getByTestId("header-more")).toBeVisible();
  await expect(page.getByTestId("pane-visibility")).toHaveCount(0);
});

test("审批的三个选项、带说明的拒绝、AskUserQuestion", async ({ page }) => {
  await connectTo(page, TOOLS);
  await expect(state(page)).toContainText("空闲");
  expect((await probe(page)).guiLog).toContain("connect m8ded9f28b056");
  // CoDeck's single-key shortcuts (j / k) stay out of the input box.
  await page.getByTestId("connect-input").pressSequentially("jk");
  await expect(page.getByTestId("connect-input")).toHaveValue("jk");

  await send(page, "MOCK-TOOL please");
  const card = page.getByTestId("permission-card");
  await expect(card).toContainText("touch mock-tool.txt");
  await expect(card.getByTestId("tool-call")).toHaveAttribute("data-tool", "Bash");
  await expect(card.getByTestId("allow-always")).toContainText("always allow access to");
  await expect(card.getByTestId("allow")).toHaveText("1. 允许");
  await expect(card.getByTestId("deny")).toBeVisible();
  // Waiting: Enter does not send, it moves focus to the card.
  await page.getByTestId("connect-input").fill("先别发");
  await page.getByTestId("connect-input").press("Enter");
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
  // A main-line question is answered on its transcript card; the input area only points there.
  await expect(page.getByTestId("question-hint")).toBeVisible();
  await expect(page.getByTestId("question-card")).toHaveCount(0);
  await page.evaluate(() => {
    const sid = "a7c63e92-0008-4000-8000-000000000008";
    const input = {
      questions: [
        {
          question: "Which color?",
          header: "Color",
          multiSelect: false,
          options: [
            { label: "Red", description: "warm" },
            { label: "Blue", description: "cool" },
          ],
        },
      ],
    };
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
        blocks: [
          {
            kind: "toolCall",
            toolUseId: "toolu_mock0002",
            name: "AskUserQuestion",
            inputJson: JSON.stringify(input),
            inputTruncated: false,
            result: null,
            subagentId: null,
            workflowRunId: null,
            notificationNodeId: null,
          },
        ],
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

test("忙时 Enter 插话（和终端里一样进 Claude Code 的队列）；数字键作答；Dock 角标；AI 识别的选择；Esc 中止；ccrpc 退出后重连", async ({
  page,
}) => {
  await connectTo(page, RICH);
  await expect(state(page)).toContainText("空闲");
  // The header says the state once, in one word, with 「Disconnect」 where 「Connect」 was; the other connection controls sit at
  // the foot of the input box, the send button at its right end.
  const header = page.getByTestId("header");
  await expect(header.getByTestId("live-state")).toHaveText("空闲");
  await expect(header.getByTestId("connect-disconnect")).toBeVisible();
  await expect(header.getByTestId("connect-controls")).toHaveCount(0);
  const box = (await page.getByTestId("connect-input").boundingBox())!;
  expect((await page.getByTestId("connect-controls").boundingBox())!.y).toBeGreaterThan(box.y + box.height - 4);
  const sendButton = (await page.getByRole("button", { name: "发送" }).boundingBox())!;
  expect(box.x + box.width - (sendButton.x + sendButton.width)).toBeLessThan(4);
  // Codex style: the permission mode at the left of the foot, the model at the right before the send button.
  const mode = (await page.getByTestId("connect-mode").boundingBox())!;
  const model = (await page.getByTestId("connect-model").boundingBox())!;
  expect(mode.x - box.x).toBeLessThan(24);
  expect(sendButton.x - (model.x + model.width)).toBeLessThan(16);
  // The input box floats over the transcript, whose end stays clear of it.
  const transcript = page.getByTestId("transcript");
  await transcript.evaluate((e) => (e.scrollTop = e.scrollHeight));
  const rows = transcript.locator("[data-row]:not([data-folded])");
  const lastRow = (await rows.nth((await rows.count()) - 1).boundingBox())!;
  const panelBox = (await page.getByTestId("composer").boundingBox())!;
  expect(lastRow.y + lastRow.height).toBeLessThan(panelBox.y);
  // Background tasks: the running one only; finished ones on request.
  await expect(page.getByTestId("tasks")).toHaveText("1 个后台任务");
  await page.getByTestId("tasks").click();
  const tasks = page.getByTestId("tasks-list").locator("li");
  await expect(tasks).toHaveCount(1);
  await expect(tasks).toContainText("pnpm test:e2e --watch");
  await page.getByTestId("tasks-done").click();
  await expect(tasks).toHaveCount(2);
  await expect(tasks.nth(1)).toContainText("为限流中间件补集成测试");
  // A Subagent opens its Subagent Run in the side panel.
  await tasks.nth(1).getByTestId("task-open").click();
  await expect(page.getByTestId("tasks-list")).toHaveCount(0);
  await expect(page.getByTestId("panel-breadcrumb").getByTestId("crumb")).toHaveText([
    "test-engineer · 为限流中间件补集成测试",
  ]);
  await page.getByTestId("panel-close").click();
  // The LCP Live Session waits (CoDeck's Live data).
  await expect.poll(async () => (await probe(page)).badge).toBe(1);

  await send(page, "改一下 README");
  await expect(state(page)).toContainText("工作中");
  // Claude Code's progress line is under the last message, in the transcript.
  await expect(page.getByTestId("transcript").getByTestId("progress")).toContainText("Thinking");

  await expect(page.getByTestId("connect-input")).toHaveAttribute("placeholder", /^插话/);
  // Busy with nothing typed: the send button is a stop button.
  await expect(page.getByTestId("stop")).toBeVisible();
  await expect(page.getByTestId("send")).toHaveCount(0);
  // An image goes along with a steer too.
  await pasteImage(page);
  await expect(page.getByTestId("images").locator("img")).toHaveCount(1);
  await send(page, "做完再跑一下测试");
  await expect
    .poll(() => lastSent(page, "steer"))
    .toMatchObject({ text: "做完再跑一下测试", images: ["/mock/Caches/dev.joy.codeck/images/1.png"] });
  await expect(page.getByTestId("images")).toHaveCount(0);
  await expect(page.getByTestId("connect-input")).toHaveValue("");
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
  await page.getByTestId("connect-input").press("Enter");
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
  await page.getByTestId("connect-input").press("Escape");
  expect(await lastSent(page, "abort")).toBeTruthy();

  // The model drafted product feedback: the card shows it; 「不发送」 dismisses it, sending stays in the terminal.
  const draft = page.getByTestId("feedback-draft");
  await expect(draft).toContainText("Claude 起草了一条产品反馈：Edit 之后没有重新读文件就改了第二处");
  await expect(draft).toContainText("在终端里按 2 才会发送");
  await expect(draft.getByTestId("feedback-draft-details")).toHaveCount(0);
  await draft.getByTestId("feedback-draft-toggle").click();
  await expect(draft.getByTestId("feedback-draft-details")).toContainText("同一文件连续两次 Edit");
  await draft.getByTestId("feedback-draft-dismiss").click();
  expect(await lastSent(page, "dismiss_feedback_draft")).toBeTruthy();
  await expect(draft).toHaveCount(0);
  await expect(page.getByTestId("proc-exit")).toContainText("退出码 1");
  await expect(page.getByTestId("connect-input")).toHaveCount(0);

  await page.getByTestId("reconnect").click();
  await expect(page.getByTestId("proc-exit")).toHaveCount(0);
  await expect(state(page)).toContainText("空闲");
  expect((await probe(page)).guiLog).toContain("reconnect mui0000000001");
});

test("Pane 里换了 Session（/clear）时选中跟着切换；复制排查信息；Disconnect", async ({ page }) => {
  await connectTo(page, AUTH);
  await expect(state(page)).toContainText("空闲");
  await send(page, "/clear");
  await expect(page.getByTestId("header").getByRole("heading", { name: JWT })).toBeVisible();
  await expect(page.getByTestId("connect-panel")).toBeVisible();

  await page.getByTestId("header-more").click();
  await page.getByRole("menuitem", { name: "复制排查信息" }).click();
  const copied = await page.evaluate(() => window.__cvCalls!.filter((c) => c.method === "copyText").pop()?.args[0]);
  expect(copied).toMatch(
    /^holder_id: mclear0000001\nsession_id: c04e77a2-0003-4000-8000-000000000003\ncwd: \/work\/lumen\nlogs: \S+$/,
  );

  await page.getByTestId("connect-disconnect").click();
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  await expect(page.getByTestId("connect-controls")).toHaveCount(0);
});

test("关闭：空闲且没有后台任务时直接结束 Session 与 Pane，并 Disconnect", async ({ page }) => {
  await connectTo(page, TURNS);
  await expect(state(page)).toContainText("空闲");
  await page.getByTestId("connect-close").click();
  await expect(page.getByTestId("connect-close-confirm")).toHaveCount(0);
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  expect(await lastSent(page, "exit")).toMatchObject({ close_pane: true, stop_tasks: true });
  expect((await probe(page)).sent.some((s) => s.msg.type === "abort")).toBe(false);
  // The Pane is gone from `ccrpc ls`: nothing to connect.
  await expect(page.getByTestId("connect-button")).toHaveCount(0);
  await expect(page.getByTestId("connect-disconnect")).toHaveCount(0);
});

test("关闭：还有后台任务时先确认，取消则什么也不发", async ({ page }) => {
  await connectTo(page, RICH);
  await expect(state(page)).toContainText("空闲");
  await page.getByTestId("connect-close").click();
  const confirm = page.getByTestId("connect-close-confirm");
  await expect(confirm).toContainText("停掉 1 个后台任务");
  await confirm.getByRole("button", { name: "取消" }).click();
  await expect(confirm).toHaveCount(0);
  expect(await lastSent(page, "exit")).toBeUndefined();
  await expect(page.getByTestId("connect-panel")).toBeVisible();

  await page.getByTestId("connect-close").click();
  await page.getByTestId("connect-close-ok").click();
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  expect(await lastSent(page, "exit")).toMatchObject({ close_pane: true, stop_tasks: true });
});

test("关闭：忙时确认后先中止本轮再结束", async ({ page }) => {
  await connectTo(page, RICH);
  await send(page, "改一下 README");
  await expect(state(page)).toContainText("工作中");
  await page.getByTestId("connect-close").click();
  await expect(page.getByTestId("connect-close-confirm")).toContainText("中止本轮、停掉 1 个后台任务");
  await page.getByTestId("connect-close-ok").click();
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  const types = (await probe(page)).sent.map((s) => s.msg.type);
  expect(types.slice(types.indexOf("abort"))).toEqual(["abort", "exit"]);
});

test("新建 Session：选目录后选中它，进索引前显示占位且可以发送", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("new-session").click();
  await expect(page.getByTestId("transcript-new")).toHaveText("新 Session，发送第一条提示词后显示对话");
  await expect(page.getByTestId("header")).toContainText("新 Session");
  await expect(page.getByTestId("header")).toContainText("/tmp/mock-new");
  await expect(state(page)).toContainText("空闲");
  await expect(page.getByTestId("connect-input")).toBeEnabled();
  expect((await probe(page)).guiLog).toContain("new /tmp/mock-new");
});

test("clay 里刚打开、未进索引的 Session 以占位行列在最上方，可选中、Connect，按 cwd 参与 Project 筛选", async ({
  page,
}) => {
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
  await page.getByTestId("connect-button").click();
  await expect(page.getByTestId("connect-panel")).toBeVisible();
  await expect(state(page)).toContainText("空闲");
  await expect(page.getByTestId("connect-input")).toBeEnabled();

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

test("斜杠命令：输入 / 列出补全，Tab 补全；内置命令走 slash，自定义命令走 prompt，打开的面板可关闭", async ({
  page,
}) => {
  await page.goto("/");
  await page.locator('[data-testid=session-row][data-session-id="c1a70000-0000-4000-8000-0000000000c1"]').click();
  await page.getByTestId("connect-button").click();
  await expect(state(page)).toContainText("空闲");
  const input = page.getByTestId("connect-input");
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

test("设置 › ccrpc：ccrpc 路径、doctor 的失败项、日志目录；分析器的 Key、Endpoint、模型可在这里设置", async ({
  page,
}) => {
  await page.goto("/");
  await page.keyboard.press("Meta+,");
  await page.getByRole("tab", { name: "ccrpc" }).click();
  await expect(page.getByTestId("set-ccrpc")).toContainText("/mock/bin/ccrpc");
  await expect(page.getByTestId("set-ccrpc")).toContainText("CODECK_CCRPC");
  await expect(page.getByTestId("set-doctor").locator('[data-level="fail"]')).toContainText("ccrpc 插件未启用");
  await expect(page.getByTestId("set-ccrpc-logs")).toContainText("/mock/Logs/dev.joy.codeck/ccrpc");

  // The analyzer: a key from the login shell until one is saved here, with an endpoint and model.
  const status = page.getByTestId("analyzer-status");
  await expect(status).toHaveText("已开启 · Key 来自登录 shell 的 CCRPC_ANALYZER_API_KEY（…abcd）");
  await expect(page.getByTestId("analyzer-base-url")).toHaveAttribute(
    "placeholder",
    "https://api.deepseek.com/anthropic",
  );
  await page.getByTestId("analyzer-key").fill("sk-ant-test-9876");
  await page.getByTestId("analyzer-base-url").fill("https://api.anthropic.com");
  await page.getByTestId("analyzer-model").fill("claude-haiku-4-5");
  await page.getByTestId("analyzer-save").click();
  await expect(status).toHaveText("已开启 · Key 保存在这里（…9876）");
  await expect(page.getByTestId("analyzer-key")).toHaveValue("");
  await expect(page.getByTestId("analyzer-base-url")).toHaveValue("https://api.anthropic.com");
  await expect(page.getByTestId("set-analyzer")).toContainText("之后新建的连接生效");
  // Saving again with the key box empty keeps the key; clearing it goes back to the shell's.
  await page.getByTestId("analyzer-save").click();
  await expect(status).toHaveText("已开启 · Key 保存在这里（…9876）");
  await page.getByTestId("analyzer-clear-key").click();
  await expect(status).toContainText("来自登录 shell");
  expect((await probe(page)).analyzerSaves).toBe(3);

  // 「测试」 tries the values in the form without saving them: the model typed here, a key typed here.
  const box = page.getByTestId("set-analyzer");
  await page.getByTestId("analyzer-test").click();
  await expect(box).toContainText("可用：claude-haiku-4-5 用时 2.3 秒");
  await page.getByTestId("analyzer-key").fill("sk-bad");
  await page.getByTestId("analyzer-test").click();
  await expect(box).toContainText("不可用：分析器请求失败：http status: 401");
  await expect(page.getByTestId("analyzer-key")).toHaveValue("sk-bad");
  await expect(status).toContainText("来自登录 shell");
  expect((await probe(page)).analyzerSaves).toBe(3);
});

test("设置 › ccrpc › 启动参数：选目录、填参数、保存；参数为空的目录保存时去掉，可删除", async ({ page }) => {
  await page.goto("/");
  await page.keyboard.press("Meta+,");
  await page.getByRole("tab", { name: "ccrpc" }).click();
  const box = page.getByTestId("set-launch-rules");
  await expect(box.getByTestId("launch-rule")).toHaveCount(0);
  await box.getByTestId("launch-rule-add").click();
  await expect(box.getByTestId("launch-rule")).toContainText("/tmp/mock-new");
  // The picker returning a directory already listed adds nothing.
  await box.getByTestId("launch-rule-add").click();
  await expect(box.getByTestId("launch-rule")).toHaveCount(1);
  await box.getByTestId("launch-rule-args").fill("--add-dir=/tmp/extra");
  await box.getByTestId("launch-rules-save").click();
  await expect(box).toContainText("已保存，之后新建的 Session 生效");

  // Saved: back after reopening the settings.
  await page.keyboard.press("Escape");
  await page.keyboard.press("Meta+,");
  await page.getByRole("tab", { name: "ccrpc" }).click();
  await expect(box.getByTestId("launch-rule-args")).toHaveValue("--add-dir=/tmp/extra");

  // Blank arguments drop the rule on saving; removing it does too.
  await box.getByTestId("launch-rule-args").fill("  ");
  await box.getByTestId("launch-rules-save").click();
  await expect(box.getByTestId("launch-rule")).toHaveCount(0);
  await box.getByTestId("launch-rule-add").click();
  await box.getByTestId("launch-rule-remove").click();
  await expect(box.getByTestId("launch-rule")).toHaveCount(0);
});

test("设置 › ccrpc › 新开的 Pane：默认显示，选「隐藏」即保存", async ({ page }) => {
  await page.goto("/");
  await page.keyboard.press("Meta+,");
  await page.getByRole("tab", { name: "ccrpc" }).click();
  const box = page.getByTestId("set-hide-new-panes");
  await expect(box.getByTestId("hide-new-panes-show")).toHaveAttribute("aria-checked", "true");
  await box.getByTestId("hide-new-panes-hide").click();
  await expect(box.getByTestId("hide-new-panes-hide")).toHaveAttribute("aria-checked", "true");
  await expect(box.getByTestId("hide-new-panes-show")).toHaveAttribute("aria-checked", "false");
  expect((await probe(page)).hideNewPanes).toBe(true);
});

test("ccrpc ls --watch 只在有 Live Session 或连接时运行，恢复时立即重新开始", async ({ page }) => {
  await page.goto("/");
  await open(page, RICH);
  await expect(page.getByTestId("connect-button")).toBeVisible();
  const ids = await page.evaluate(() =>
    // Indexed rows only: the clay-new placeholder is Live through `ccrpc ls`, not the mock data.
    [...document.querySelectorAll("[data-testid=session-row]:not([data-pending]) [data-testid=live-dot]")].map((d) =>
      d.closest("[data-session-id]")!.getAttribute("data-session-id")!,
    ),
  );
  expect(ids.length).toBeGreaterThan(0);
  await page.evaluate((list) => list.forEach((id) => window.__cvMock!.setLive(id, null)), ids);
  await expect(page.getByTestId("live-dot")).toHaveCount(0);
  // With nothing Live the watch stops and what it said last is dropped.
  await expect(page.getByTestId("connect-button")).toHaveCount(0);
  await expect.poll(async () => (await probe(page)).watching).toBe(false);
  const before = (await probe(page)).watchStarts;
  // 「新建 Session」 works without an `ccrpc ls` answer.
  await expect(page.getByTestId("new-session")).toBeVisible();

  await page.evaluate(
    (id) => window.__cvMock!.setLive(id, { status: "idle", rawStatus: "idle", waitingFor: null, pid: 1 }),
    ids[0]!,
  );
  await expect.poll(async () => (await probe(page)).watchStarts, { timeout: 1500 }).toBe(before + 1);
  expect((await probe(page)).watching).toBe(true);
  await expect(page.getByTestId("connect-button")).toBeVisible();
});

test("ccrpc 在连接命令返回前就退出：显示退出横幅而不是一直「连接中」", async ({ page }) => {
  await connectTo(page, REFUSED);
  await expect(page.getByTestId("proc-exit")).toContainText("退出码 1");
  await expect(page.getByTestId("proc-exit")).toContainText("Holder 版本太旧");
  // Only the banner says it: no input box under it.
  await expect(page.getByTestId("connect-input")).toHaveCount(0);
  await expect(page.getByTestId("reconnect")).toBeVisible();
});

test("Session 在终端里退出后：输入区只剩一行提示，头部重新出现「Connect」，resume 后可再次 Connect", async ({
  page,
}) => {
  await connectTo(page, "（无标题）");
  await expect(page.getByTestId("session-exit")).toContainText("Session 已结束");
  await expect(page.getByTestId("connect-input")).toHaveCount(0);
  await expect(page.getByTestId("connect-disconnect")).toHaveCount(0);
  // Still in `ccrpc ls` (resumed in the terminal): no Resume, connecting again replaces the ended connection.
  await expect(page.getByTestId("session-exit-resume")).toHaveCount(0);
  await page.getByTestId("connect-button").click();
  await expect.poll(async () => (await probe(page)).guiLog.filter((l) => l === "connect mresumed00001").length).toBe(2);
  expect((await probe(page)).guiLog).toContain("close (replaced)");
  await expect(page.getByTestId("session-exit")).toHaveCount(1);
});

test("Resume：已结束、不在 clay 里的 Session 在新 Pane 里重新运行并 Connect", async ({ page }) => {
  const redis = "91d5be38-0004-4000-8000-000000000004";
  await page.goto("/");
  await open(page, "排查 Redis 连接池泄漏");
  const header = page.getByTestId("header");
  await expect(header.getByTestId("connect-button")).toHaveCount(0);
  const resume = header.getByTestId("resume-button");
  await expect(resume).toHaveAttribute("title", /从 Main Line 末尾继续/);
  await resume.click();
  // The same Session, connected: no 「新 Session」 row, the header and the input area as for Connect.
  await expect(header.getByRole("heading", { name: "排查 Redis 连接池泄漏" })).toBeVisible();
  await expect(header.getByTestId("connect-disconnect")).toBeVisible();
  await expect(page.getByTestId("connect-input")).toBeEnabled();
  await expect(header.getByTestId("resume-button")).toHaveCount(0);
  await expect(page.locator(`[data-testid=session-row][data-pending][data-session-id="${redis}"]`)).toHaveCount(0);
  await expect(page.locator(`[data-testid=session-row][data-session-id="${redis}"]`)).toHaveCount(1);
  expect((await probe(page)).guiLog).toContain(`resume ${redis}`);
});

test("Resume：ccrpc 在 ready 里就给出 Session ID 时同样连上（不停在「Resume 中…」）", async ({ page }) => {
  await page.goto("/");
  await open(page, "排查 Redis 连接池泄漏");
  await page.evaluate(() => {
    (window.__cvCcrpc as unknown as { resumeNamedInReady: boolean }).resumeNamedInReady = true;
  });
  await page.getByTestId("resume-button").click();
  await expect(page.getByTestId("header").getByTestId("connect-disconnect")).toBeVisible();
  await expect(page.getByTestId("resume-pending")).toHaveCount(0);
  await expect(page.getByTestId("connect-input")).toBeEnabled();
});

test("Resume 失败：claude 找不到 Session 时标题栏写明原因，新开的 Pane 关掉", async ({ page }) => {
  await page.goto("/");
  await open(page, JWT);
  await page.getByTestId("resume-button").click();
  // Until the failure is dealt with (the screen read, the Pane closed) it cannot be started again.
  await expect(page.getByTestId("resume-pending")).toBeVisible();
  await expect(page.getByTestId("resume-button")).toHaveCount(0);
  const error = page.getByTestId("header").getByTestId("connect-error");
  await expect(error).toHaveText(
    "Resume 失败：No conversation found with session ID: c04e77a2-0003-4000-8000-000000000003",
  );
  // Read from the Pane, then the Pane closed and the connection dropped: Resume can be tried again.
  expect(await lastSent(page, "get_screen")).toBeTruthy();
  expect(await lastSent(page, "exit")).toMatchObject({ close_pane: true });
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  await expect(page.getByTestId("resume-button")).toBeEnabled();
});

test("Session 已结束的卡片上直接 Resume", async ({ page }) => {
  await page.goto("/");
  await open(page, "排查 Redis 连接池泄漏");
  await page.getByTestId("resume-button").click();
  await expect(page.getByTestId("connect-input")).toBeEnabled();
  // `/exit` ends claude (its Pane stays, out of `ccrpc ls`): the card, with Resume on it.
  await send(page, "/exit");
  const card = page.getByTestId("session-exit");
  await expect(card).toContainText("Session 已结束");
  await card.getByTestId("session-exit-resume").click();
  await expect(page.getByTestId("session-exit")).toHaveCount(0);
  await expect(page.getByTestId("connect-input")).toBeEnabled();
  await expect(page.getByTestId("connect-disconnect")).toBeVisible();
  expect((await probe(page)).guiLog.filter((l) => l === "resume 91d5be38-0004-4000-8000-000000000004")).toHaveLength(2);
});

test("目录信任框是选择卡片", async ({ page }) => {
  await connectTo(page, TRUST);
  const card = page.getByTestId("choice-card");
  await expect(card).toContainText("trust");
  await expect(card.getByTestId("ai-badge")).toHaveCount(0);
  await card.getByTestId("choice-option").filter({ hasText: "Yes, I trust this folder" }).click();
  await expect(card).toHaveCount(0);
  await expect(state(page)).toContainText("空闲");
  expect(await lastSent(page, "answer_choice")).toMatchObject({ option: "Yes, I trust this folder" });
});

test("粘贴的图片随提示词发出", async ({ page }) => {
  await connectTo(page, TURNS);
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
  await connectTo(page, TURNS);
  await expect(state(page)).toContainText("空闲");
  await page.getByTestId("connect-input").fill("还没写完的草稿");
  await pasteImage(page);
  await expect(page.getByTestId("images").locator("img")).toHaveCount(1);
  await open(page, TOOLS);
  await page.getByTestId("connect-button").click();
  await expect(state(page)).toContainText("空闲");
  await expect(page.getByTestId("connect-input")).toHaveValue("");
  await expect(page.getByTestId("images")).toHaveCount(0);
  await open(page, TURNS);
  await expect(page.getByTestId("connect-input")).toHaveValue("还没写完的草稿");
  await expect(page.getByTestId("images").locator("img")).toHaveCount(1);

  // And across a restart: back when the Session is connected again.
  await page.reload();
  await open(page, TURNS);
  await page.getByTestId("connect-button").click();
  await expect(page.getByTestId("connect-input")).toHaveValue("还没写完的草稿");
  await expect(page.getByTestId("images").locator("img")).toHaveCount(1);
  // Sent, it is gone for good.
  await page.getByTestId("connect-input").press("Enter");
  await expect(page.getByTestId("connect-input")).toHaveValue("");
  await page.reload();
  await open(page, TURNS);
  await page.getByTestId("connect-button").click();
  await expect(state(page)).toContainText("空闲");
  await expect(page.getByTestId("connect-input")).toHaveValue("");
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

test("模型与思考等级：底栏右侧的选择器（照 Claude Code Desktop），空闲时可切换，切换后立即显示", async ({ page }) => {
  await connectTo(page, RICH);
  await expect(state(page)).toContainText("空闲");
  const trigger = page.getByTestId("connect-model");
  await expect(trigger).toBeEnabled();
  await expect(trigger).toHaveText(/^Opus 5\.5/);
  // The recording has no effort mark: no Effort entry.
  await trigger.click();
  const menu = page.getByTestId("model-menu");
  await expect(menu.getByRole("menuitemcheckbox", { name: "Opus" })).toHaveAttribute("aria-checked", "true");
  await expect(page.getByTestId("effort-trigger")).toHaveCount(0);
  await menu.getByRole("menuitemcheckbox", { name: "Fable" }).click();
  await expect.poll(() => lastSent(page, "set_model")).toMatchObject({ model: "fable" });
  await expect(trigger).toHaveText(/^Fable 5\.1\s*High/);

  // Effort in its submenu; xhigh reads Extra.
  await trigger.click();
  await page.getByTestId("effort-trigger").hover();
  const effort = page.getByTestId("effort-menu");
  await expect(effort.getByRole("menuitemcheckbox", { name: "High" })).toHaveAttribute("aria-checked", "true");
  // By keyboard: the test's pointer jumps (a real one stays inside the submenu's grace area).
  await effort.getByRole("menuitemcheckbox", { name: "Extra" }).focus();
  await page.keyboard.press("Enter");
  await expect.poll(() => lastSent(page, "set_effort")).toMatchObject({ level: "xhigh" });
  await expect(page.getByTestId("connect-effort")).toHaveText("Extra");

  // Any other model by name.
  await trigger.click();
  await page.getByTestId("other-model-trigger").hover();
  await page.getByTestId("other-model").fill("claude-opus-4-6");
  await page.getByTestId("other-model").press("Enter");
  await expect.poll(() => lastSent(page, "set_model")).toMatchObject({ model: "claude-opus-4-6" });
  await expect(trigger).toHaveText(/^Opus 4\.6/);

  // Busy: Claude Code takes /model only when idle.
  await send(page, "改一下 README");
  await expect(state(page)).toContainText("工作中");
  await expect(trigger).toBeDisabled();
});
