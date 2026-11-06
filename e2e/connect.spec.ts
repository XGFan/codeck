import { expect, test, type Locator, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import { composeEnd, composeStart } from "./ime";

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
// resumed → the untitled Session (it ends right after connecting, as if `/exit` in the terminal),
// rewind → the Redis Session (listed only for the rewind tests: elsewhere it is the Session not in clay).

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
  recordsOpened: number;
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

/** The header's 「···」 menu, opened: Connect, Disconnect and the Pane are in it. */
async function headerMenu(page: Page) {
  const menu = page.getByTestId("header-menu");
  if (!(await menu.isVisible())) await page.getByTestId("header-more").click();
  return menu;
}
/** Clicks an item of the header's 「···」 menu. */
const pick = async (page: Page, testId: string) => (await headerMenu(page)).getByTestId(testId).click();
/** Checks an item of the header's 「···」 menu, then closes the menu. */
async function inMenu(page: Page, testId: string, check: (item: Locator) => Promise<void>) {
  await check((await headerMenu(page)).getByTestId(testId));
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("header-menu")).toHaveCount(0);
}

async function connectTo(page: Page, title: string) {
  await page.goto("/");
  await open(page, title);
  await pick(page, "connect-button");
  await expect(page.getByTestId("connect-panel")).toBeVisible();
}

async function send(page: Page, text: string, mod?: "Meta") {
  const input = page.getByTestId("connect-input");
  await input.fill(text);
  await input.press(mod ? `${mod}+Enter` : "Enter");
}

const state = (page: Page) => page.getByTestId("connect-state");

// Connected by hand here (「Connect」 would race 自动 Connect); 自动 Connect has a test of its own.
test.beforeEach(({ page }) =>
  page.addInitScript(
    () => localStorage.getItem("codeck.autoConnect") ?? localStorage.setItem("codeck.autoConnect", '{"on":false}'),
  ),
);

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

test("「···」菜单：ccrpc ls 里的 Live Session 可 Connect，插件未报到时禁用，其他 Session 没有；标题栏里没有", async ({
  page,
}) => {
  await page.goto("/");
  await open(page, RICH);
  await expect(page.getByTestId("header").getByRole("button", { name: "Connect" })).toHaveCount(0);
  await inMenu(page, "connect-button", (b) => expect(b).toBeEnabled());
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);

  await open(page, NO_PLUGIN);
  await inMenu(page, "connect-button", async (b) => {
    await expect(b).toHaveAttribute("data-disabled");
    await expect(b).toContainText("ccrpc 插件未加载，重启 claude 后可用");
  });

  await open(page, NOT_LISTED);
  await inMenu(page, "connect-button", (b) => expect(b).toHaveCount(0));
});

test("「···」菜单：clay 里的 Session 不论是否 Connect 都可隐藏 / 显示 Pane，不在 clay 里的没有", async ({ page }) => {
  await page.goto("/");
  await open(page, RICH);
  await inMenu(page, "pane-visibility", (b) => expect(b).toContainText("隐藏 Pane"));
  await pick(page, "pane-visibility");
  await inMenu(page, "pane-visibility", (b) => expect(b).toContainText("显示 Pane"));
  const [hide] = (await probe(page)).paneHides;
  expect(hide.hidden).toBe(true);

  await pick(page, "connect-button");
  await expect(page.getByTestId("connect-panel")).toBeVisible();
  await inMenu(page, "pane-visibility", (b) => expect(b).toContainText("显示 Pane"));
  await pick(page, "pane-visibility");
  await inMenu(page, "pane-visibility", (b) => expect(b).toContainText("隐藏 Pane"));
  expect((await probe(page)).paneHides).toEqual([hide, { holderId: hide.holderId, hidden: false }]);

  await open(page, NOT_LISTED);
  await inMenu(page, "pane-visibility", (b) => expect(b).toHaveCount(0));
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
  // Design A: full-width rows, each with its number key; the card says what it waits for.
  await expect(card.getByTestId("allow")).toHaveText("1允许");
  await expect(card).toContainText("等待审批");
  const allowBox = (await card.getByTestId("allow").boundingBox())!;
  const denyBox = (await card.getByTestId("deny").boundingBox())!;
  expect(Math.abs(allowBox.width - denyBox.width)).toBeLessThan(1);
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
  // Sent: the Session is synced at once, not after the watcher's debounce.
  expect(
    await page.evaluate(() => window.__cvCalls!.filter((c) => c.method === "syncSession").map((c) => c.args[0])),
  ).toContain("a7c63e92-0008-4000-8000-000000000008");

  await send(page, "MOCK-TOOL again");
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
  const q = page.locator("[data-testid=ask][data-answering]");
  await expect(q).toContainText("Which color?");
  // The card in view (the transcript follows its end): no hint below; scrolled out of view, the hint is back.
  await expect(page.getByTestId("question-hint")).toHaveCount(0);
  // A short window, scrolled to the top: the card is under the input area.
  await page.setViewportSize({ width: 1280, height: 340 });
  await page.getByTestId("transcript").evaluate((e) => (e.scrollTop = 0));
  await expect(page.getByTestId("question-hint")).toBeVisible();
  await page.getByTestId("question-jump").click();
  await expect(page.getByTestId("question-hint")).toHaveCount(0);
  await page.setViewportSize({ width: 1280, height: 720 });
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

test("自动 Connect：打开可 Connect 的 Session 就连上，Disconnect 后不再自动连，再次打开才会；可在设置里关掉", async ({
  page,
}) => {
  // The default (on): once, not again on the reload below.
  await page.addInitScript(() => {
    if (sessionStorage.getItem("e2e-default")) return;
    sessionStorage.setItem("e2e-default", "1");
    localStorage.removeItem("codeck.autoConnect");
  });
  await page.goto("/");
  // Visited once, so its header comes from the cache when opened again below (the header is not remounted).
  await open(page, NOT_LISTED);
  // On by default.
  await open(page, RICH);
  await expect(page.getByTestId("connect-panel")).toBeVisible();
  await expect(state(page)).toContainText("空闲");
  await pick(page, "connect-disconnect");
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  await page.waitForTimeout(800);
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  await inMenu(page, "connect-button", (b) => expect(b).toBeVisible());
  // Opened again: connected again.
  await open(page, NOT_LISTED);
  await open(page, RICH);
  await expect(page.getByTestId("connect-panel")).toBeVisible();
  // Opened while connected already (another Session connected meanwhile): a Disconnect stays.
  await open(page, TOOLS);
  await expect(page.getByTestId("connect-panel")).toBeVisible();
  await open(page, RICH);
  await pick(page, "connect-disconnect");
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  await page.waitForTimeout(800);
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  // Not a Session whose plugin has not reported.
  await open(page, NO_PLUGIN);
  await page.waitForTimeout(800);
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);

  // Off in Settings › 通用, and remembered.
  await page.getByTestId("settings-button").click();
  await page.getByRole("tab", { name: "通用" }).click();
  await expect(page.getByTestId("auto-connect-true")).toHaveAttribute("aria-checked", "true");
  await page.getByTestId("auto-connect-false").click();
  await page.keyboard.press("Escape");
  await open(page, TURNS);
  await page.waitForTimeout(800);
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  await inMenu(page, "connect-button", (b) => expect(b).toBeEnabled());
  await page.reload();
  await page.getByTestId("settings-button").click();
  await page.getByRole("tab", { name: "通用" }).click();
  await expect(page.getByTestId("auto-connect-false")).toHaveAttribute("aria-checked", "true");
});

/** The `uuid` ccrpc answered the n-th `prompt` of a recording with (the prompt's transcript entry). */
function recordedPromptUuid(name: string, n: number): string {
  return readFileSync(`src/mocks/data/connect/${name}.jsonl`, "utf8")
    .split("\n")
    .filter((l) => l.trim())
    .map((l) => JSON.parse(l))
    .filter((l) => l.dir === "recv" && l.msg?.type === "response" && l.msg.command === "prompt")[n]!.msg.data.uuid;
}

test("发出提示词：输入框立即清空，对话末尾先是跳动省略号的占位气泡，提示词进索引后换成它；发送失败时占位去掉、文字回到输入框", async ({
  page,
}) => {
  await connectTo(page, TOOLS);
  await expect(state(page)).toContainText("空闲");
  const input = page.getByTestId("connect-input");
  const pending = page.getByTestId("pending-prompt");

  // Refused: the placeholder goes, the text is back in the box, the error says why.
  await page.evaluate(() => ((window.__cvCcrpc as unknown as { failNext: string }).failNext = "prompt"));
  await send(page, "MOCK-TOOL please");
  await expect(input).toHaveValue("");
  await expect(pending).toBeVisible();
  await expect(pending).toHaveCount(0);
  await expect(input).toHaveValue("MOCK-TOOL please");
  await expect(page.getByTestId("connect-error")).toContainText("发送失败");

  // Sent: dots only (not the text: the transcript draws it as indexed) until the prompt is indexed.
  await input.press("Enter");
  await expect(input).toHaveValue("");
  await expect(pending.locator(".typing-dot")).toHaveCount(3);
  await expect(pending).not.toContainText("MOCK-TOOL");
  await expect.poll(() => lastSent(page, "prompt")).toMatchObject({ text: "MOCK-TOOL please" });
  await page.waitForTimeout(500);
  await expect(pending).toBeVisible();
  const uuid = recordedPromptUuid("tools", 0);
  await page.evaluate(
    ({ sid, id }) => {
      window.__cvMock!.appendNode(sid, {
        id,
        timestampMs: Date.now(),
        hidden: false,
        inherited: false,
        body: { kind: "userPrompt", text: "MOCK-TOOL please", images: [], origin: { kind: "human" } },
      });
      window.__cvMock!.emit("sessionsChanged", { changed: [sid], removed: [], projectsChanged: false });
    },
    { sid: "a7c63e92-0008-4000-8000-000000000008", id: uuid },
  );
  // Replaced by the prompt, which settles in where the placeholder was.
  await expect(pending).toHaveCount(0);
  const bubble = page.getByTestId("user-bubble").filter({ hasText: "MOCK-TOOL please" });
  await expect(bubble).toBeVisible();
  await expect(bubble).toHaveAttribute("data-arrived", "true");
});

test("发出消息后对话记录滚到底，看得到占位气泡", async ({ page }) => {
  await connectTo(page, RICH);
  await expect(state(page)).toContainText("空闲");
  const transcript = page.getByTestId("transcript");
  const fromBottom = () => transcript.evaluate((e) => e.scrollHeight - e.scrollTop - e.clientHeight);
  // Reading further up.
  await transcript.evaluate((e) => (e.scrollTop = 0));
  await expect.poll(fromBottom).toBeGreaterThan(100);
  await send(page, "接着做");
  await expect.poll(fromBottom).toBeLessThan(4);
  await expect(page.getByTestId("pending-prompt")).toBeInViewport();
  await expect(page.getByTestId("new-messages")).toHaveCount(0);
});

test("多选题可以同时勾选项与「其他」，一起发出", async ({ page }) => {
  await connectTo(page, TOOLS);
  await expect(state(page)).toContainText("空闲");
  // A background Subagent's question: answered on its own card in the input area.
  await page.evaluate(() =>
    window.__cvCcrpc!.emit({
      type: "question",
      request_id: "toolu_e2e_multi",
      agent_id: "a1b2c3d4e5f6",
      questions: [
        {
          question: "防住哪些情况？",
          header: "目标",
          multiSelect: true,
          options: [
            { label: "系统睡眠", description: "后台任务中断" },
            { label: "锁屏", description: "屏幕保持亮着" },
            { label: "Teams 显示离开", description: "状态保持有空" },
          ],
        },
      ],
    }),
  );
  const q = page.getByTestId("question-card");
  const option = (label: string) => q.getByTestId("question-option").filter({ hasText: label });
  const otherRow = q.getByTestId("question-other-row");
  await option("系统睡眠").click();
  await option("锁屏").click();
  await q.getByTestId("question-other").fill("前三类有什么不同？");
  // Writing keeps the options picked, picking one more keeps the text.
  await expect(option("系统睡眠")).toHaveAttribute("aria-pressed", "true");
  await expect(option("锁屏")).toHaveAttribute("aria-pressed", "true");
  await expect(otherRow).toHaveAttribute("data-on", "true");
  await option("Teams 显示离开").click();
  await option("Teams 显示离开").click();
  await expect(otherRow).toHaveAttribute("data-on", "true");
  // Clicking the caption writes in the row, it does not untick it.
  await otherRow.getByText("其他：").click();
  await expect(otherRow).toHaveAttribute("data-on", "true");
  await expect(q.getByTestId("question-other")).toBeFocused();
  // 其他's own check box sets the text aside and back.
  await q.getByTestId("question-other-toggle").click();
  await expect(otherRow).not.toHaveAttribute("data-on");
  await q.getByTestId("question-other-toggle").click();
  await expect(otherRow).toHaveAttribute("data-on", "true");
  await q.getByTestId("submit-answer").click();
  await expect
    .poll(() => lastSent(page, "answer_question"))
    .toMatchObject({
      request_id: "toolu_e2e_multi",
      answers: [{ options: ["系统睡眠", "锁屏"], other: "前三类有什么不同？" }],
    });
});

test("先聊聊：带上已答的题（没答的为 null）发出 chat，焦点回到输入框；每道题都答了时不能先聊聊", async ({ page }) => {
  await connectTo(page, TOOLS);
  await expect(state(page)).toContainText("空闲");
  await page.evaluate(() =>
    window.__cvCcrpc!.emit({
      type: "question",
      request_id: "toolu_e2e_chat",
      agent_id: "a1b2c3d4e5f6",
      questions: [
        { question: "用哪个颜色？", header: "颜色", multiSelect: false, options: [{ label: "红" }, { label: "蓝" }] },
        {
          question: "要哪些尺寸？",
          header: "尺寸",
          multiSelect: true,
          options: [{ label: "S" }, { label: "M" }, { label: "L" }],
        },
      ],
    }),
  );
  const q = page.getByTestId("question-card");
  const option = (label: string) => q.getByTestId("question-option").filter({ hasText: label });
  const chat = q.getByTestId("chat-about-question");
  await option("红").click();
  await option("S").click();
  await expect(chat).toBeDisabled();
  await option("红").click();
  await option("L").click();
  await chat.click();
  await expect
    .poll(() => lastSent(page, "answer_question"))
    .toEqual(
      expect.objectContaining({ request_id: "toolu_e2e_chat", chat: true, answers: [null, { options: ["S", "L"] }] }),
    );
  await expect(q).toHaveCount(0);
  await expect(page.getByTestId("connect-input")).toBeFocused();
});

test("回复前按 Esc 撤回的提示词：从对话里拿掉、文字回到输入框，显示系统消息时标「已撤回」，再发同样的话照常显示", async ({
  page,
}) => {
  await connectTo(page, TOOLS);
  await expect(state(page)).toContainText("空闲");
  const append = (id: string) =>
    page.evaluate((id) => {
      const sid = "a7c63e92-0008-4000-8000-000000000008";
      window.__cvMock!.appendNode(sid, {
        id,
        timestampMs: Date.now(),
        hidden: false,
        inherited: false,
        body: { kind: "userPrompt", text: "改成两行\n的那个", images: [], origin: { kind: "human" } },
      });
      window.__cvMock!.emit("sessionsChanged", { changed: [sid], removed: [], projectsChanged: false });
    }, id);
  const bubble = (id: string) => page.locator(`[data-node-id="${id}"]`);
  await append("e2e-taken-back");
  await expect(bubble("e2e-taken-back")).toBeVisible();
  // Esc before any reply: Claude Code put it back in its input box.
  await page.evaluate(() =>
    window.__cvCcrpc!.emit({
      type: "turn_end",
      prompt_id: null,
      trigger: "prompt",
      last_text: null,
      error: null,
      aborted: true,
      restored_text: "改成两行\n的那个",
      background_tasks: [],
      duration_ms: 300,
      usage: { input_tokens: 0, output_tokens: 0, cache_read_input_tokens: 0, cache_creation_input_tokens: 0 },
      api_cost: 0,
    }),
  );
  await expect(bubble("e2e-taken-back")).toHaveCount(0);
  await expect(page.getByTestId("connect-input")).toHaveValue("改成两行\n的那个");
  // With the hidden entries: dimmed, marked, nothing to rewind to.
  await page.getByTestId("header-more").click();
  await page.getByRole("menuitemcheckbox", { name: "显示系统消息" }).click();
  await expect(bubble("e2e-taken-back").getByTestId("prompt-withdrawn")).toHaveText("已撤回");
  await expect(bubble("e2e-taken-back").getByTestId("rewind-button")).toHaveCount(0);
  await page.getByTestId("header-more").click();
  await page.getByRole("menuitemcheckbox", { name: "显示系统消息" }).click();
  // The same words sent again (a new turn): shown.
  await page.evaluate(() => window.__cvCcrpc!.emit({ type: "state", state: "busy" }));
  await append("e2e-sent-again");
  await expect(bubble("e2e-sent-again")).toBeVisible();
  await expect(bubble("e2e-taken-back")).toHaveCount(0);
});

test("作答时选项旁显示 preview：先是第一项的，悬停、键盘焦点所在或选中的那项的", async ({ page }) => {
  await connectTo(page, TOOLS);
  await expect(state(page)).toContainText("空闲");
  await page.evaluate(() =>
    window.__cvCcrpc!.emit({
      type: "question",
      request_id: "toolu_e2e_preview",
      agent_id: "a1b2c3d4e5f6",
      questions: [
        {
          question: "侧栏的整体结构选哪种？",
          header: "侧栏结构",
          multiSelect: false,
          options: [
            {
              label: "单栏分组",
              description: "可折叠的 Project 组",
              preview: "▾ codeck      +\n   ● 重新设计  2分\n▸ infra (217) +",
            },
            {
              label: "三栏",
              description: "Project 栏 + Session 栏",
              preview: "│全部    │● 重新设计│\n│● codeck│  修复诊断│",
            },
            { label: "保持平铺", description: "改动最小" },
          ],
        },
      ],
    }),
  );
  const q = page.getByTestId("question-card");
  const option = (label: string) => q.getByTestId("question-option").filter({ hasText: label });
  const shown = q.getByTestId("option-preview").locator("pre:not([aria-hidden])");
  await expect(shown).toContainText("▾ codeck");
  await option("三栏").hover();
  await expect(shown).toContainText("│● codeck│");
  await option("保持平铺").hover();
  await expect(shown).toHaveText("此项没有预览");
  // Picked: its preview stays once the pointer leaves.
  await option("三栏").click();
  await q.getByTestId("question-other").hover();
  await expect(shown).toContainText("│● codeck│");
  // Keyboard focus shows the focused option's; the pointer passing over another and off leaves it.
  await option("单栏分组").focus();
  await expect(shown).toContainText("▾ codeck");
  await option("三栏").hover();
  await expect(shown).toContainText("│● codeck│");
  await q.getByTestId("question-other").hover();
  await expect(shown).toContainText("▾ codeck");
});

test("输入框里的斜杠命令高亮；/add-dir 补全目录、@ 补全文件，支持 ../", async ({ page }) => {
  await connectTo(page, RICH);
  await expect(state(page)).toContainText("空闲");
  const input = page.getByTestId("connect-input");
  const highlight = page.getByTestId("command-highlight");
  // A known command at the start is drawn in color; not inside a sentence, nor an unknown name.
  await input.fill("/compact 只留结论");
  await expect(highlight.locator("span.text-accent")).toHaveText("/compact");
  await page.screenshot({ path: "test-results/command-highlight.png" });
  // Long enough to scroll: the copy wraps and scrolls as the textarea does.
  await input.fill(`/compact ${"只留结论，".repeat(400)}`);
  await input.evaluate((t) => (t.scrollTop = t.scrollHeight));
  const sizes = () =>
    input.evaluate((t) => {
      const m = t.parentElement!.querySelector<HTMLElement>("[data-testid=command-highlight]")!;
      return { t: [t.clientWidth, t.scrollHeight, t.scrollTop], m: [m.clientWidth, m.scrollHeight, m.scrollTop] };
    });
  await expect.poll(async () => (await sizes()).t[2]).toBeGreaterThan(0);
  await expect.poll(async () => (await sizes()).m).toEqual((await sizes()).t);
  await input.fill("先 /compact");
  await expect(highlight).toHaveCount(0);
  // Nothing else drawn in the box (a stray "0" once showed under the text).
  expect(
    await input.evaluate((t) =>
      [...t.parentElement!.childNodes].filter((n) => n.nodeType === Node.TEXT_NODE).map((n) => n.textContent),
    ),
  ).toEqual([]);
  await input.fill("/nope x");
  await expect(highlight).toHaveCount(0);
  // A plugin's command by its name alone (`kit:review` here) is one too.
  await input.fill("/review 这次的改动");
  await expect(highlight.locator("span.text-accent")).toHaveText("/review");
  // Composing with an input method (pinyin) after it: the command keeps its color, the marked text underlined.
  await input.fill("/review ");
  await composeStart(input, ["/review z", "/review zhe'ci"]);
  await expect(highlight.locator("span.text-accent")).toHaveText("/review");
  await expect(highlight.locator("[data-marked]")).toHaveText("zhe'ci");
  await expect(input).toHaveClass(/text-transparent/);
  await composeEnd(input, "/review 这次");
  await expect(highlight.locator("span.text-accent")).toHaveText("/review");
  await expect(highlight.locator("[data-marked]")).toHaveCount(0);
  // In the middle, over a selection: only what is composed is underlined.
  await input.fill("/review 甲乙");
  await input.evaluate((t: HTMLTextAreaElement) => t.setSelectionRange(8, 9));
  await composeStart(input, ["/review z乙", "/review zhe乙"]);
  await expect(highlight.locator("[data-marked]")).toHaveText("zhe");
  await composeEnd(input, "/review 这乙");
  await expect(highlight.locator("[data-marked]")).toHaveCount(0);

  // The Session's cwd is /work/demo: its directories, hidden ones only after a `.`.
  const menu = page.getByTestId("path-menu");
  const options = menu.getByTestId("path-option");
  await input.fill("/add-dir ");
  await expect(highlight.locator("span.text-accent")).toHaveText("/add-dir");
  await expect(options).toHaveText(["apps/", "clusters/"]);
  await input.pressSequentially("a");
  await expect(options).toHaveText(["apps/"]);
  // Tab completes; the list goes on inside it.
  await input.press("Tab");
  await expect(input).toHaveValue("/add-dir apps/");
  await expect(options).toHaveText(["apps/kestrel/"]);
  // Up a level as typed.
  await input.fill("/add-dir ..");
  await expect(options).toHaveText(["../"]);
  await input.fill("/add-dir ../in");
  await expect(options).toHaveText(["../infra/", "../infra-modules/"]);
  await page.screenshot({ path: "test-results/dir-menu.png" });
  // Enter takes a pick moved to with ↑↓, otherwise sends what is typed.
  await input.press("ArrowDown");
  await input.press("Enter");
  await expect(input).toHaveValue("/add-dir ../infra-modules/");
  await expect(menu).toHaveCount(0);
  await input.press("Enter");
  await expect.poll(() => lastSent(page, "slash")).toMatchObject({ command: "add-dir ../infra-modules/" });
  // Esc hides the list only.
  await input.fill("/add-dir ../");
  await expect(menu).toBeVisible();
  await input.press("Escape");
  await expect(menu).toHaveCount(0);
  await expect(input).toHaveValue("/add-dir ../");

  // `@` in a prompt: files too, the word the caret is in.
  await input.fill("看看 @");
  await expect(options).toHaveText(["apps/", "Cargo.toml", "clusters/", "README.md"]);
  await input.pressSequentially("ap");
  await input.press("Tab");
  await expect(input).toHaveValue("看看 @apps/");
  await expect(options).toHaveText(["apps/kestrel/"]);
  await input.press("Tab");
  await expect(input).toHaveValue("看看 @apps/kestrel/");
  await expect(options).toHaveText(["apps/kestrel/main.go"]);
  // A file ends the word: a space after it, the list gone.
  await input.press("Tab");
  await expect(input).toHaveValue("看看 @apps/kestrel/main.go ");
  await expect(menu).toHaveCount(0);
  // In the middle of the text, with the caret after `@RE`.
  await input.fill("改 @RE 里的说明");
  for (let i = 0; i < 5; i++) await input.press("ArrowLeft");
  await expect(options).toHaveText(["README.md"]);
  await input.press("Tab");
  await expect(input).toHaveValue("改 @README.md 里的说明");
  expect(await input.evaluate((t: HTMLTextAreaElement) => t.selectionStart)).toBe(13);
  // Not after a letter (an address), nor once past the word.
  await input.fill("发到 me@host");
  await expect(menu).toHaveCount(0);
});

test("忙时 Enter 插话（和终端里一样进 Claude Code 的队列）；数字键作答；Dock 角标；AI 识别的选择；Esc 中止；ccrpc 退出后重连", async ({
  page,
}) => {
  await connectTo(page, RICH);
  await expect(state(page)).toContainText("空闲");
  // The header says the state once, in one word, with 「关闭」 (Disconnect is in its 「···」 menu); the other connection
  // controls sit at the foot of the input box, the send button at its right end.
  const header = page.getByTestId("header");
  await expect(header.getByTestId("live-state")).toHaveText("空闲");
  await expect(header.getByTestId("connect-close")).toBeVisible();
  await expect(header.getByTestId("connect-disconnect")).toHaveCount(0);
  await inMenu(page, "connect-disconnect", (b) => expect(b).toBeVisible());
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
  // Background tasks: a button for each kind with running ones, with their count; each lists only its kind,
  // finished ones on request.
  await expect(page.getByTestId("tasks-shell")).toHaveText("1 Shell");
  await expect(page.getByTestId("tasks-agent")).toHaveText("1 Subagent");
  await expect(page.getByTestId("tasks-monitor")).toHaveText("1 Monitor");
  await expect(page.getByTestId("tasks-workflow")).toHaveCount(0);
  const list = page.getByTestId("tasks-list");
  const tasks = list.locator("li");
  await page.getByTestId("tasks-shell").click();
  await expect(list).toHaveAttribute("data-kind", "shell");
  await expect(tasks).toHaveCount(1);
  await expect(tasks).toContainText("pnpm test:e2e --watch");
  // A shell whose call is not in the transcript is not a link; it has no finished ones.
  await expect(list.getByTestId("task-jump")).toHaveCount(0);
  await expect(page.getByTestId("tasks-done")).toHaveCount(0);
  await page.keyboard.press("Escape");
  // A Monitor jumps to the tool call that started it.
  await page.getByTestId("tasks-monitor").click();
  await expect(list).toHaveAttribute("data-kind", "monitor");
  await expect(tasks).toHaveCount(1);
  await expect(tasks).toContainText("限流 429 日志");
  await expect(tasks).toContainText("tail -F logs/ratelimit.log");
  await list.getByTestId("task-jump").click();
  await expect(list).toHaveCount(0);
  await expect(page.locator('[data-anchor$="tu_monitor"]').first()).toBeInViewport();
  // Subagents: the running one, the finished one on request.
  await page.getByTestId("tasks-agent").click();
  await expect(list).toHaveAttribute("data-kind", "agent");
  await expect(tasks).toHaveCount(1);
  await page.getByTestId("tasks-done").click();
  await expect(tasks).toHaveCount(2);
  // A Subagent opens its Subagent Run in the side panel.
  await tasks.filter({ hasText: "为限流中间件补集成测试" }).getByTestId("task-open").click();
  await expect(list).toHaveCount(0);
  await expect(page.getByTestId("panel-breadcrumb").getByTestId("crumb")).toHaveText([
    "test-engineer · 为限流中间件补集成测试",
  ]);
  await page.getByTestId("panel-close").click();
  // A Teammate is listed under a task id of its own: it opens its Subagent Run by its description.
  await page.getByTestId("tasks-agent").click();
  await tasks.filter({ hasText: "补限流文档" }).getByTestId("task-open").click();
  await expect(page.getByTestId("panel-breadcrumb").getByTestId("crumb")).toHaveText(["writer · 补限流文档"]);
  await page.getByTestId("panel-close").click();
  // Its only Shell stopped: the popover stays open and says so; closed, the Shell button is gone.
  await page.getByTestId("tasks-shell").click();
  await tasks.getByRole("button", { name: "停止" }).click();
  await expect.poll(() => lastSent(page, "stop_task")).toMatchObject({ task_id: "bg-run" });
  await expect(tasks).toContainText("已停止");
  await expect(page.getByTestId("tasks-shell")).toHaveText("0 Shell");
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("tasks-shell")).toHaveCount(0);
  await expect(page.getByTestId("tasks-agent")).toHaveText("1 Subagent");
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
  // An image goes along with a steer too, where it was pasted.
  await page.getByTestId("connect-input").fill("做完再跑一下测试");
  await pasteImage(page);
  await expect(page.getByTestId("image-chip")).toHaveCount(1);
  await page.getByTestId("connect-input").press("Enter");
  await expect
    .poll(() => lastSent(page, "steer"))
    .toMatchObject({ parts: [{ text: "做完再跑一下测试" }, { image: "/mock/Caches/dev.joy.codeck/images/1.png" }] });
  await expect(page.getByTestId("image-chip")).toHaveCount(0);
  await expect(page.getByTestId("connect-input")).toHaveValue("");
  // The TUI's own queue, as the terminal shows it; sent at once on request (ctrl+enter there). A message from another
  // agent reads as its sender and first line, not its tag.
  await expect(page.getByTestId("queue-item")).toHaveText([
    "做完再跑一下测试[Image #1]",
    "；codeck-99：合并前先拉一下 main。",
  ]);
  // ↑ on the box's first row takes what was queued back to edit, its image too (↑ in the terminal), before what is
  // typed; the message from another agent stays queued. On a lower row of a long line ↑ only moves the caret.
  const input = page.getByTestId("connect-input");
  await expect(input).toHaveAttribute("placeholder", /^按 ↑ 取回排队的消息/);
  await input.fill("很长的一行".repeat(60));
  await input.press("End");
  await input.press("ArrowUp");
  await page.waitForTimeout(200);
  expect(await lastSent(page, "edit_queue")).toBeUndefined();
  await input.fill("再补一句");
  await input.press("ArrowUp");
  await expect.poll(() => lastSent(page, "edit_queue")).toBeDefined();
  await expect(input).toHaveValue(/^做完再跑一下测试[^\n]+\n再补一句$/);
  // The caret right after what came back.
  expect(await input.evaluate((el: HTMLTextAreaElement) => el.selectionStart === el.value.indexOf("\n"))).toBe(true);
  await expect(page.getByTestId("image-chip")).toHaveCount(1);
  await expect(page.getByTestId("queue-item")).toHaveText(["codeck-99：合并前先拉一下 main。"]);
  // Nothing of it left to take back: ↑ only moves the caret.
  await input.press("ArrowUp");
  await page.waitForTimeout(200);
  expect((await probe(page)).sent.filter((s) => s.msg.type === "edit_queue").length).toBe(1);
  await input.fill("");
  await expect(page.getByTestId("image-chip")).toHaveCount(0);
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
  await expect(card.getByTestId("allow")).toHaveText("1Yes");
  await expect(card.getByTestId("deny")).toHaveText("3No");
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

  await pick(page, "connect-disconnect");
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  await expect(page.getByTestId("connect-controls")).toHaveCount(0);
});

test("关闭：空闲且没有后台任务时直接结束 Session 与 Pane，并 Disconnect", async ({ page }) => {
  await connectTo(page, TURNS);
  await expect(state(page)).toContainText("空闲");
  await page.getByTestId("connect-close").click();
  await expect(page.getByTestId("connect-close-confirm")).toHaveCount(0);
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  expect(await lastSent(page, "exit")).toMatchObject({ close_pane: true, stop_tasks: false });
  expect((await probe(page)).sent.some((s) => s.msg.type === "abort")).toBe(false);
  // The Pane is gone from `ccrpc ls`: nothing to connect.
  await inMenu(page, "connect-button", (b) => expect(b).toHaveCount(0));
  await inMenu(page, "connect-disconnect", (b) => expect(b).toHaveCount(0));
});

test("关闭：Claude Code 退出时说还有后台任务（CoDeck 不知道的，如它自己的 monitor），先确认再停掉", async ({
  page,
}) => {
  await connectTo(page, TURNS);
  await expect(state(page)).toContainText("空闲");
  await page.evaluate(() => ((window.__cvCcrpc as unknown as { monitorRunning: boolean }).monitorRunning = true));
  await page.getByTestId("connect-close").click();
  const confirm = page.getByTestId("connect-close-confirm");
  await expect(confirm).toContainText("将停掉后台任务，结束 Session 并关闭它的 Pane。");
  expect(await lastSent(page, "exit")).toMatchObject({ stop_tasks: false });
  await expect(page.getByTestId("connect-panel")).toBeVisible();
  await expect(page.getByTestId("connect-error")).toHaveCount(0);
  // Cancelled: nothing more is sent, and closing again asks again.
  await confirm.getByRole("button", { name: "取消" }).click();
  await expect(confirm).toHaveCount(0);
  await page.getByTestId("connect-close").click();
  await expect(confirm).toBeVisible();
  expect((await probe(page)).sent.filter((s) => s.msg.type === "exit")).toHaveLength(2);
  await page.getByTestId("connect-close-ok").click();
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  expect(await lastSent(page, "exit")).toMatchObject({ close_pane: true, stop_tasks: true });
});

test("关闭：还有后台任务时先确认，取消则什么也不发", async ({ page }) => {
  await connectTo(page, RICH);
  await expect(state(page)).toContainText("空闲");
  await page.getByTestId("connect-close").click();
  const confirm = page.getByTestId("connect-close-confirm");
  await expect(confirm).toContainText("停掉 3 个后台任务");
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
  await expect(page.getByTestId("connect-close-confirm")).toContainText("中止本轮、停掉 3 个后台任务");
  await page.getByTestId("connect-close-ok").click();
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  const types = (await probe(page)).sent.map((s) => s.msg.type);
  expect(types.slice(types.indexOf("abort"))).toEqual(["abort", "exit"]);
});

test("新建 Session：选目录后选中它，进索引前显示占位且可以发送", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("new-session").click();
  await page.getByTestId("new-session-other").click();
  await expect(page.getByTestId("transcript-new")).toHaveText("新 Session，发送第一条提示词后显示对话");
  await expect(page.getByTestId("header")).toContainText("新 Session");
  await expect(page.getByTestId("header")).toContainText("/tmp/mock-new");
  await expect(state(page)).toContainText("空闲");
  await expect(page.getByTestId("connect-input")).toBeEnabled();
  expect((await probe(page)).guiLog).toContain("new /tmp/mock-new");
});

const NEW_ID = "0e5e0000-0000-4000-8000-0000000000ee";
const openId = (page: Page) => page.evaluate(() => window.__cvStore!.getState().sessionId);

test("新建 Session：点下即打开页面，启动中可以打字，claude 报到后草稿与焦点都还在", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => (window.__cvCcrpc!.newStartMs = 1500));
  await page.getByTestId("new-session").click();
  await page.getByTestId("new-session-other").click();
  // Before claude names the Session: its page, on the connection's id.
  await expect(page.getByTestId("transcript-new")).toHaveText("新 Session，发送第一条提示词后显示对话");
  await expect(page.getByTestId("header")).toContainText("新 Session");
  await expect(page.getByTestId("header")).toContainText("/tmp/mock-new");
  await expect(state(page)).toHaveText("启动中");
  expect(await openId(page)).toMatch(/^mock/);
  const input = page.getByTestId("connect-input");
  await expect(input).toBeFocused();
  await page.keyboard.type("先写一半");
  // Named: the page is the Session's, nothing typed is lost and typing goes on.
  await expect.poll(() => openId(page)).toBe(NEW_ID);
  await expect(state(page)).toHaveText("空闲");
  await expect(input).toHaveValue("先写一半");
  await expect(input).toBeFocused();
  await page.keyboard.type("，再写完");
  await expect(input).toHaveValue("先写一半，再写完");
});

test("新建 Session：启动中去看别的 Session，claude 报到后不会被切回去", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => (window.__cvCcrpc!.newStartMs = 1000));
  await page.getByTestId("new-session").click();
  await page.getByTestId("new-session-other").click();
  await expect(state(page)).toHaveText("启动中");
  await open(page, RICH);
  const other = await openId(page);
  await page.waitForTimeout(1500);
  expect(await openId(page)).toBe(other);
  await expect(page.getByTestId("header").getByRole("heading", { name: RICH })).toBeVisible();
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
  await pick(page, "connect-button");
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
  await pick(page, "connect-button");
  await expect(state(page)).toContainText("空闲");
  const input = page.getByTestId("connect-input");
  const menu = page.getByTestId("slash-menu");
  const options = menu.getByTestId("slash-option");

  // `/` alone lists every command; the list scrolls with the pick.
  await input.fill("/");
  await expect(options).toHaveCount(11);
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

test("设置 › ccrpc：分析记录从新到旧列出学会的对话框与失败原因，可打开记录目录", async ({ page }) => {
  await page.goto("/");
  await page.keyboard.press("Meta+,");
  await page.getByRole("tab", { name: "ccrpc" }).click();
  const rows = page.getByTestId("analyzer-record");
  await expect(rows).toHaveCount(2);
  await expect(rows.nth(0)).toContainText("学会");
  await expect(rows.nth(0)).toContainText("Computer Use wants to control these apps");
  await expect(rows.nth(1)).toContainText("失败");
  await expect(rows.nth(1)).toContainText("分析器认为屏幕上没有等待作答的菜单");
  await expect(page.getByTestId("set-analyzer-records")).toContainText("/mock/.local/share/ccrpc/analyses");
  await page.getByTestId("analyzer-records-open").click();
  await expect.poll(async () => (await probe(page)).recordsOpened).toBe(1);
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
  await inMenu(page, "connect-button", (b) => expect(b).toBeVisible());
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
  await inMenu(page, "connect-button", (b) => expect(b).toHaveCount(0));
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
  await inMenu(page, "connect-button", (b) => expect(b).toBeVisible());
});

test("ccrpc 在连接命令返回前就退出：显示退出横幅而不是一直「连接中」", async ({ page }) => {
  await connectTo(page, REFUSED);
  await expect(page.getByTestId("proc-exit")).toContainText("退出码 1");
  await expect(page.getByTestId("proc-exit")).toContainText("Holder 版本太旧");
  // Only the banner says it: no input box under it.
  await expect(page.getByTestId("connect-input")).toHaveCount(0);
  await expect(page.getByTestId("reconnect")).toBeVisible();
});

test("Session 在终端里退出后：输入区只剩一行提示，「···」菜单里重新出现「Connect」，resume 后可再次 Connect", async ({
  page,
}) => {
  await connectTo(page, "（无标题）");
  await expect(page.getByTestId("session-exit")).toContainText("Session 已结束");
  await expect(page.getByTestId("connect-input")).toHaveCount(0);
  await inMenu(page, "connect-disconnect", (b) => expect(b).toHaveCount(0));
  // Still in `ccrpc ls` (resumed in the terminal): no Resume, connecting again replaces the ended connection.
  await expect(page.getByTestId("session-exit-resume")).toHaveCount(0);
  await pick(page, "connect-button");
  await expect.poll(async () => (await probe(page)).guiLog.filter((l) => l === "connect mresumed00001").length).toBe(2);
  expect((await probe(page)).guiLog).toContain("close (replaced)");
  await expect(page.getByTestId("session-exit")).toHaveCount(1);
});

test("Resume：已结束、不在 clay 里的 Session 在新 Pane 里重新运行并 Connect", async ({ page }) => {
  const redis = "91d5be38-0004-4000-8000-000000000004";
  await page.goto("/");
  await open(page, "排查 Redis 连接池泄漏");
  const header = page.getByTestId("header");
  await inMenu(page, "connect-button", (b) => expect(b).toHaveCount(0));
  const resume = header.getByTestId("resume-button");
  await expect(resume).toHaveAttribute("title", /从 Main Line 末尾继续/);
  await resume.click();
  // The same Session, connected: no 「新 Session」 row, the header and the input area as for Connect.
  await expect(header.getByRole("heading", { name: "排查 Redis 连接池泄漏" })).toBeVisible();
  await expect(header.getByTestId("connect-close")).toBeVisible();
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
  await expect(page.getByTestId("header").getByTestId("connect-close")).toBeVisible();
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
  await expect(page.getByTestId("connect-close")).toBeVisible();
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

/** The input box's words, the image tokens left out. */
const words = (page: Page) =>
  page
    .getByTestId("connect-input")
    .evaluate((e: HTMLTextAreaElement) => e.value.replace(/\u2062[^\u2063]*\u2063/g, "|"));

test("粘贴的图片插在光标处，悬停预览，随提示词按位置发出", async ({ page }) => {
  await connectTo(page, TURNS);
  await expect(state(page)).toContainText("空闲");
  const input = page.getByTestId("connect-input");
  await input.fill("对比 和 MOCK-SLOW please");
  // The caret after 「对比 」.
  await input.press("Meta+ArrowLeft");
  for (let i = 0; i < 3; i++) await input.press("ArrowRight");
  await pasteImage(page);
  const chips = page.getByTestId("image-chip");
  await expect(chips).toHaveCount(1);
  expect(await words(page)).toBe("对比 |和 MOCK-SLOW please");
  // No previews above the box: hovering the token shows its image (a data: URL, the app's CSP allows no other).
  await expect(page.getByTestId("images")).toHaveCount(0);
  await chips.first().hover();
  const preview = page.getByTestId("image-preview").locator("img");
  await expect(preview).toBeVisible();
  expect(await preview.getAttribute("src")).toMatch(/^data:image\/png;base64,/);
  await expect(page.getByTestId("image-preview")).toContainText("图片 1 · 1×1");
  // The pointer can move onto the preview (it stays), and a click on its image shows it large.
  await page.getByTestId("image-preview-open").hover();
  await page.waitForTimeout(400);
  await expect(page.getByTestId("image-preview")).toBeVisible();
  await page.getByTestId("image-preview-open").click();
  await expect(page.getByTestId("image-large")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("image-large")).toHaveCount(0);
  // Away from both: it goes.
  await page.mouse.move(0, 0);
  await expect(page.getByTestId("image-preview")).toHaveCount(0);
  await expect(input).toBeFocused();
  await input.press("Meta+ArrowRight");
  await input.press("Enter");
  await expect
    .poll(() => lastSent(page, "prompt"))
    .toMatchObject({
      parts: [
        { text: "对比 " },
        { image: "/mock/Caches/dev.joy.codeck/images/1.png" },
        { text: "和 MOCK-SLOW please" },
      ],
    });
  await expect(chips).toHaveCount(0);
});

test("只有图片、没有文字也能发出", async ({ page }) => {
  await connectTo(page, TURNS);
  await expect(state(page)).toContainText("空闲");
  await pasteImage(page);
  await expect(page.getByTestId("image-chip")).toHaveCount(1);
  await expect(page.getByTestId("send")).toBeEnabled();
  await page.getByTestId("connect-input").press("Enter");
  await expect
    .poll(() => lastSent(page, "prompt"))
    .toEqual({
      type: "prompt",
      parts: [{ image: "/mock/Caches/dev.joy.codeck/images/1.png" }],
      id: expect.any(String),
    });
  await expect(page.getByTestId("image-chip")).toHaveCount(0);
});

test("图片标记是一整块：光标跳过、⌫ 整块删掉，拖到别处后按新位置重新编号", async ({ page }) => {
  await connectTo(page, TURNS);
  await expect(state(page)).toContainText("空闲");
  const input = page.getByTestId("connect-input");
  await input.fill("甲乙");
  await input.press("Meta+ArrowLeft");
  await input.press("ArrowRight");
  await pasteImage(page);
  await input.press("Meta+ArrowRight");
  await pasteImage(page);
  const chips = page.getByTestId("image-chip");
  await expect(chips).toHaveCount(2);
  expect(await words(page)).toBe("甲|乙|");
  // A gap between the block and the word before it.
  const gap = await page.evaluate(() => {
    const mirror = document.querySelector("[data-testid=composer-mirror]")!;
    const chip = mirror.querySelector("[data-chip]")!.getBoundingClientRect();
    const r = document.createRange();
    const first = mirror.firstChild!;
    r.setStart(first, 0);
    r.setEnd(first, 1);
    return chip.left - r.getBoundingClientRect().right;
  });
  expect(gap).toBeGreaterThanOrEqual(3);
  // ← from the end steps over the second token at once; ⌫ there takes the first whole.
  await input.press("Meta+ArrowRight");
  await input.press("ArrowLeft");
  await input.press("ArrowLeft");
  const caret = () =>
    input.evaluate((e: HTMLTextAreaElement) =>
      e.value.slice(0, e.selectionStart).replace(/\u2062[^\u2063]*\u2063/g, "|"),
    );
  expect(await caret()).toBe("甲|");
  await input.press("Backspace");
  expect(await words(page)).toBe("甲乙|");
  await expect(chips).toHaveCount(1);
  await expect(chips.first()).toHaveAttribute("data-n", "1");

  // Another one at the start, then dragged to the end: the two swap numbers.
  await input.press("Meta+ArrowLeft");
  await pasteImage(page);
  await expect(chips).toHaveCount(2);
  expect(await words(page)).toBe("|甲乙|");
  const paths = async () => ((await probe(page)) as unknown as { images: number }).images;
  expect(await paths()).toBe(3);
  const first = (await chips.first().boundingBox())!;
  await page.mouse.move(first.x + first.width / 2, first.y + first.height / 2);
  await page.mouse.down();
  await page.mouse.move(first.x + first.width + 8, first.y + first.height / 2, { steps: 3 });
  await expect(page.getByTestId("image-drop")).toBeVisible();
  // Between 「甲」 and 「乙」.
  const second = (await chips.nth(1).boundingBox())!;
  await page.mouse.move((first.x + first.width + second.x) / 2, first.y + first.height / 2, { steps: 4 });
  await page.mouse.up();
  expect(await words(page)).toBe("甲|乙|");
  await input.press("Meta+ArrowRight");
  await input.press("Enter");
  await expect
    .poll(() => lastSent(page, "prompt"))
    .toMatchObject({
      parts: [
        { text: "甲" },
        { image: "/mock/Caches/dev.joy.codeck/images/3.png" },
        { text: "乙" },
        { image: "/mock/Caches/dev.joy.codeck/images/2.png" },
      ],
    });
});

test("没发出的输入跟着 Session，切换、重启后仍在，发出后清掉", async ({ page }) => {
  await connectTo(page, TURNS);
  await expect(state(page)).toContainText("空闲");
  await page.getByTestId("connect-input").fill("还没写完的草稿");
  await pasteImage(page);
  await expect(page.getByTestId("image-chip")).toHaveCount(1);
  await open(page, TOOLS);
  await pick(page, "connect-button");
  await expect(state(page)).toContainText("空闲");
  await expect(page.getByTestId("connect-input")).toHaveValue("");
  await expect(page.getByTestId("image-chip")).toHaveCount(0);
  await open(page, TURNS);
  expect(await words(page)).toBe("还没写完的草稿|");
  await expect(page.getByTestId("image-chip")).toHaveCount(1);

  // And across a restart: back when the Session is connected again.
  await page.reload();
  await open(page, TURNS);
  await pick(page, "connect-button");
  expect(await words(page)).toBe("还没写完的草稿|");
  await expect(page.getByTestId("image-chip")).toHaveCount(1);
  // Sent, it is gone for good.
  await page.getByTestId("connect-input").press("Enter");
  await expect(page.getByTestId("connect-input")).toHaveValue("");
  await page.reload();
  await open(page, TURNS);
  await pick(page, "connect-button");
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

/** Connects the Redis Session through the rewind recording (out of `ccrpc ls` unless asked for). */
async function connectRewind(page: Page) {
  await page.goto("/");
  await page.evaluate(() => (window.__cvCcrpc as unknown as { list: (name: string) => void }).list("rewind"));
  await open(page, NOT_LISTED);
  await pick(page, "connect-button");
  await expect(page.getByTestId("connect-panel")).toBeVisible();
  await expect(state(page)).toContainText("空闲");
}
const REDIS = "91d5be38-0004-4000-8000-000000000004";
const U0_TEXT = "线上 Redis 连接数持续增长，帮我排查连接池泄漏。";
const U1_TEXT = "session.rs 第 44 行看起来没问题？";
const U2_TEXT = "换个思路，看看 spawn 出去的任务";
const promptRow = (page: Page, id: string) =>
  page.getByTestId("transcript").locator(`[data-node-id="${id}"][data-kind=userPrompt]`);
const rowOf = (page: Page, id: string) => page.locator(`[data-row]:has([data-node-id="${id}"][data-kind=userPrompt])`);
const sentCount = async (page: Page, type: string) =>
  (await probe(page)).sent.filter((x) => x.msg.type === type).length;

/** Hovers a prompt and clicks its 「回退」; the command must name that prompt (the replay matches by type only). */
async function rewindOn(page: Page, id: string) {
  const row = promptRow(page, id);
  await row.getByTestId("user-bubble").hover();
  const b = row.getByTestId("rewind-button");
  await expect(b).toHaveCSS("opacity", "1");
  const before = await sentCount(page, "rewind");
  await b.click();
  await expect.poll(() => sentCount(page, "rewind")).toBe(before + 1);
  expect(await lastSent(page, "rewind")).toMatchObject({ uuid: id });
}

/** The prompt sent after the rewind is indexed: the Main Line forks before the rewound prompt. */
const indexFork = (page: Page) =>
  page.evaluate(
    ({ sid, text }) => {
      window.__cvMock!.forkMain(sid, "91d5-u1", {
        id: "91d5-u2",
        timestampMs: Date.now(),
        hidden: false,
        inherited: false,
        body: { kind: "userPrompt", text, images: [], origin: { kind: "human" } },
      });
      window.__cvMock!.emit("sessionsChanged", { changed: [sid], removed: [], projectsChanged: false });
    },
    { sid: REDIS, text: U2_TEXT },
  );

test("回退：/rewind 进入选回退点，Esc 与「取消」退出；点「回退」后卡片写明代码与警告，取消收起", async ({ page }) => {
  await connectRewind(page);
  const buttons = page.getByTestId("transcript").getByTestId("rewind-button");
  await expect(buttons).toHaveCount(2);
  await expect(buttons.first()).toHaveCSS("opacity", "0");

  await send(page, "/rewind");
  const pick = page.getByTestId("rewind-pick");
  await expect(pick).toContainText("选一条提示词，回退到它之前");
  await expect(pick).toContainText("点提示词下方的「回退」 · Esc 取消");
  await expect(page.getByTestId("connect-input")).toHaveValue("");
  // Every prompt shows it without hovering; the transcript goes to the last one.
  for (const b of await buttons.all()) await expect(b).toHaveCSS("opacity", "1");
  await expect(page.locator('[data-anchor="91d5-u1"]')).toHaveAttribute("data-highlighted", "true");
  // Esc closing something else (⌘K search) leaves the mode on; the next one leaves it.
  await page.keyboard.press("Meta+K");
  await expect(page.getByTestId("search")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("search")).toBeHidden();
  await expect(pick).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(pick).toHaveCount(0);
  await expect(buttons.first()).toHaveCSS("opacity", "0");
  // `/checkpoint` is the same command.
  await send(page, "/checkpoint");
  await pick.getByTestId("rewind-pick-cancel").click();
  await expect(pick).toHaveCount(0);
  // Neither reaches the terminal.
  const types = (await probe(page)).sent.map((x) => x.msg.type);
  expect(types).not.toContain("slash");
  expect(types).not.toContain("prompt");

  await send(page, "/rewind");
  await rewindOn(page, "91d5-u1");
  await expect(pick).toHaveCount(0);
  const card = page.getByTestId("rewind-card");
  await expect(card).toContainText("回退到这条提示词之前");
  await expect(card.getByTestId("rewind-quote")).toHaveText(U1_TEXT);
  await expect(card.getByTestId("rewind-code-line")).toHaveText(
    "这条提示词及之后的对话会被移除；代码会恢复：src/session.rs（−12）",
  );
  await expect(card.getByTestId("rewind-warning")).toHaveText(
    "恢复的文件整份覆盖，之后的改动都会丢失；Bash 对其他文件的改动不会撤销。",
  );
  await expect(card.locator("[data-card-answer]")).toHaveText([
    "1恢复代码与对话",
    "2只恢复对话代码保持现状",
    "3只恢复代码对话保持现状",
  ]);
  // Just asked for: the card takes focus, none of its options.
  await expect(page.getByTestId("rewind-card-box")).toBeFocused();
  await card.getByTestId("rewind-cancel").click();
  await expect(card).toHaveCount(0);
  expect(await lastSent(page, "answer_rewind")).toMatchObject({ request_id: "rewind:1", cancel: true });
  await expect(state(page)).toContainText("空闲");
});

test("回退：恢复代码与对话后原提示词进输入框、之后的对话淡化，切换 Session 仍在，新提示词进索引后复原", async ({
  page,
}) => {
  await connectRewind(page);
  const u0 = promptRow(page, "91d5-u0");
  const u1 = promptRow(page, "91d5-u1");
  const card = page.getByTestId("rewind-card");
  const input = page.getByTestId("connect-input");
  const divider = page.getByTestId("rewound-divider");

  await u1.getByTestId("user-bubble").hover();
  await expect(u1.getByTestId("rewind-button")).toHaveAttribute("title", "回退到这条提示词之前");
  await rewindOn(page, "91d5-u1");
  // Esc on the card cancels the rewind (not an abort), once however often it is pressed.
  await expect(card).toBeVisible();
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");
  await expect(card).toHaveCount(0);
  expect(await lastSent(page, "answer_rewind")).toMatchObject({ cancel: true });
  expect(await sentCount(page, "answer_rewind")).toBe(1);
  expect(await lastSent(page, "abort")).toBeUndefined();
  await expect(page.getByTestId("connect-error")).toHaveCount(0);

  // The terminal's picker does not have it.
  await rewindOn(page, "91d5-u1");
  const error = page.getByTestId("connect-error");
  await expect(error).toContainText("回退失败");
  await expect(error).toContainText("终端的回退列表里没有这条提示词");

  await rewindOn(page, "91d5-u1");
  await expect(card).toBeVisible();
  await expect(error).toHaveCount(0);
  // Only while idle.
  await u0.getByTestId("user-bubble").hover();
  const b0 = u0.getByTestId("rewind-button");
  await expect(b0).toBeDisabled();
  await expect(b0).toHaveAttribute("title", "空闲时才能回退");
  await card.getByTestId("rewind-both").click();
  await expect(card).toHaveCount(0);
  expect(await lastSent(page, "answer_rewind")).toMatchObject({ request_id: "rewind:2", option: "both" });
  await expect(input).toHaveValue(U1_TEXT);
  await expect(input).toBeFocused();
  await expect(divider).toHaveText("已回退到这里，代码已恢复 · 发出新提示词后，下面这段成为另一个 Branch");
  await expect(rowOf(page, "91d5-u1")).toHaveAttribute("data-rewound", "true");
  await expect(rowOf(page, "91d5-u1")).toContainText("已回退到这里");
  await expect(rowOf(page, "91d5-u0")).not.toHaveAttribute("data-rewound");
  expect(await page.locator("[data-row][data-rewound]").count()).toBeGreaterThan(1);
  // ccrpc no longer has the rewound part on its Main Line: no 「回退」 there.
  await expect(u1.getByTestId("rewind-button")).toHaveCount(0);
  await expect(u0.getByTestId("rewind-button")).toHaveCount(1);

  // It belongs to the connection, not to what is on screen.
  await open(page, RICH);
  await open(page, NOT_LISTED);
  await expect(divider).toBeVisible();
  await expect(u1.getByTestId("rewind-button")).toHaveCount(0);

  // A new prompt: the marker stays until the transcript shows the new Branch.
  await send(page, U2_TEXT);
  await expect(input).toHaveValue("");
  await expect(state(page)).toContainText("空闲");
  await expect(divider).toBeVisible();
  await indexFork(page);
  await expect(promptRow(page, "91d5-u2")).toBeVisible();
  await expect(divider).toHaveCount(0);
  await expect(page.locator("[data-row][data-rewound]")).toHaveCount(0);

  // Only the code: nothing dimmed, a notice.
  await rewindOn(page, "91d5-u2");
  await card.getByTestId("rewind-code").click();
  await expect(card).toHaveCount(0);
  await expect(page.getByTestId("notice")).toContainText("已恢复代码");
  await expect(divider).toHaveCount(0);
  await expect(input).toHaveValue("");
});

test("回退：没有代码改动时只能恢复对话；输入框已有内容时保留，原提示词不填入", async ({ page }) => {
  await connectRewind(page);
  // The recording's earlier rounds: cancelled, not in the picker, both, a prompt, only the code.
  const card = page.getByTestId("rewind-card");
  await rewindOn(page, "91d5-u1");
  await card.getByTestId("rewind-cancel").click();
  await expect(card).toHaveCount(0);
  await rewindOn(page, "91d5-u1");
  await expect(page.getByTestId("connect-error")).toBeVisible();
  await rewindOn(page, "91d5-u1");
  await card.getByTestId("rewind-both").click();
  await expect(page.getByTestId("rewound-divider")).toBeVisible();
  await send(page, U2_TEXT);
  await indexFork(page);
  await expect(page.getByTestId("rewound-divider")).toHaveCount(0);
  await rewindOn(page, "91d5-u2");
  await card.getByTestId("rewind-code").click();
  await expect(page.getByTestId("notice")).toContainText("已恢复代码");

  const input = page.getByTestId("connect-input");
  await input.fill("先留着的草稿");
  await rewindOn(page, "91d5-u0");
  await expect(card.getByTestId("rewind-quote")).toHaveText(U0_TEXT);
  await expect(card.getByTestId("rewind-code-line")).toHaveText("这条提示词及之后的对话会被移除；代码不变");
  await expect(card.getByTestId("rewind-warning")).toHaveCount(0);
  await expect(card.locator("[data-card-answer]")).toHaveText(["1只恢复对话代码保持现状"]);
  // As on the other cards: Enter in the input box moves focus to the card, then 1 picks its first option.
  await input.press("Enter");
  await expect(card.getByTestId("rewind-conversation")).toBeFocused();
  await page.keyboard.press("1");
  await expect(card).toHaveCount(0);
  expect(await lastSent(page, "answer_rewind")).toMatchObject({ request_id: "rewind:4", option: "conversation" });
  await expect(input).toHaveValue("先留着的草稿");
  await expect(page.getByTestId("rewind-kept-draft")).toHaveText("输入框里已有内容，原提示词没有填入");
  await expect(page.getByTestId("rewound-divider")).toHaveText(
    "已回退到这里 · 发出新提示词后，下面这段成为另一个 Branch",
  );
  await expect(rowOf(page, "91d5-u0")).toHaveAttribute("data-rewound", "true");
  await expect(page.getByTestId("transcript").getByTestId("rewind-button")).toHaveCount(0);
});
