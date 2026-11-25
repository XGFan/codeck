import { expect, test } from "@playwright/test";
import { readFileSync } from "node:fs";
import path from "node:path";
import {
  connectTo,
  disableAutoConnect,
  inMenu,
  lastSent,
  open,
  pick,
  probe,
  state,
  TOOLS,
  RICH,
  NOT_LISTED,
  send,
} from "./connect-helpers";

// Connected by hand here (「Connect」 would race 自动 Connect); 自动 Connect has a test of its own.
test.beforeEach(({ page }) => disableAutoConnect(page));

/** The `turn_end` prompt ids of a recorded fixture, in order (they change when it is re-recorded). */
function recordedTurns(name: string): string[] {
  return readFileSync(path.join(import.meta.dirname, `../src/mocks/data/connect/${name}.jsonl`), "utf8")
    .split("\n")
    .filter((l) => l.trim())
    .map((l) => JSON.parse(l))
    .filter((l) => l.dir === "recv" && l.msg?.type === "turn_end")
    .map((l) => l.msg.prompt_id as string);
}

const toolsTurns = recordedTurns("tools");

const TRUST = "/review 订单列表页";

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
            code: null,
            viaCode: null,
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

test("多选题可以同时勾选项与「其他」，一起发出", async ({ page }) => {
  await connectTo(page, TOOLS);
  await expect(state(page)).toContainText("空闲");
  // A background Subagent's question: answered on its own card in the input area.
  await page.evaluate(() =>
    window.__cvCcrpc!.emit({
      type: "question",
      request_id: "toolu_e2e_multi",
      tool_use_id: "toolu_e2e_multi",
      agent_id: "a1b2c3d4e5f6",
      from_code: false,
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

test("输入区的提问卡片：题目多、窗口矮时题目在卡片里滚动，「提交」一直看得见", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 600 });
  await connectTo(page, TOOLS);
  await expect(state(page)).toContainText("空闲");
  const options = ["甲", "乙", "丙", "丁"].map((label) => ({ label, description: "说明文字" }));
  await page.evaluate(
    (options) =>
      window.__cvCcrpc!.emit({
        type: "question",
        request_id: "repl_e2e_tall",
        tool_use_id: "repl_e2e_tall",
        agent_id: null,
        from_code: true,
        questions: ["一", "二", "三"].map((n) => ({
          question: `第${n}题选哪个？`,
          header: `题${n}`,
          multiSelect: false,
          options,
        })),
      }),
    options,
  );
  const q = page.getByTestId("question-card");
  await expect(q).toContainText("第三题选哪个？");
  // More than fits: the buttons stay in sight, the questions scroll under the card's header.
  await expect(q.getByTestId("submit-answer")).toBeInViewport({ ratio: 1 });
  await expect(q.getByTestId("chat-about-question")).toBeInViewport({ ratio: 1 });
  for (const n of [0, 1, 2]) await q.locator("fieldset").nth(n).getByTestId("question-option").first().click();
  await expect(q.getByTestId("submit-answer")).toBeInViewport({ ratio: 1 });
  await q.getByTestId("submit-answer").click();
  await expect
    .poll(() => lastSent(page, "answer_question"))
    .toMatchObject({
      request_id: "repl_e2e_tall",
      answers: [{ options: ["甲"] }, { options: ["甲"] }, { options: ["甲"] }],
    });
});

test("请求卡片留到 ccrpc 报它结束（request_end），不随状态变化消失", async ({ page }) => {
  await connectTo(page, TOOLS);
  await expect(state(page)).toContainText("空闲");
  const emit = (msg: Record<string, unknown>) => page.evaluate((m) => window.__cvCcrpc!.emit(m), msg);
  await emit({ type: "state", state: "waiting", waiting: { kind: "permission", waiting_for: "permission prompt" } });
  await emit({
    type: "permission_request",
    request_id: "toolu_e2e_end",
    tool_use_id: "toolu_e2e_end",
    tool_name: "Bash",
    input: { command: "rm -rf build" },
    agent_id: null,
    from_code: false,
    options: null,
    layout_source: null,
  });
  const card = page.getByTestId("permission-card");
  await expect(card).toContainText("rm -rf build");
  // Answered in the terminal: the Session runs on, the card stays until ccrpc says the request is over.
  await emit({ type: "state", state: "busy", waiting: null });
  await page.waitForTimeout(300);
  await expect(card).toBeVisible();
  await emit({ type: "request_end", request_id: "toolu_e2e_end" });
  await expect(card).toHaveCount(0);
});

test("先聊聊：带上已答的题（没答的为 null）发出 chat，焦点回到输入框；每道题都答了时不能先聊聊", async ({ page }) => {
  await connectTo(page, TOOLS);
  await expect(state(page)).toContainText("空闲");
  await page.evaluate(() =>
    window.__cvCcrpc!.emit({
      type: "question",
      request_id: "toolu_e2e_chat",
      tool_use_id: "toolu_e2e_chat",
      agent_id: "a1b2c3d4e5f6",
      from_code: false,
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

test("作答时选项旁显示 preview：先是第一项的，悬停、键盘焦点所在或选中的那项的", async ({ page }) => {
  await connectTo(page, TOOLS);
  await expect(state(page)).toContainText("空闲");
  await page.evaluate(() =>
    window.__cvCcrpc!.emit({
      type: "question",
      request_id: "toolu_e2e_preview",
      tool_use_id: "toolu_e2e_preview",
      agent_id: "a1b2c3d4e5f6",
      from_code: false,
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

test("认不出的界面：「在终端窗口中处理」打开这个 Pane 的终端窗口，键入直达；回到输入框时窗口提示，Disconnect 后窗口关闭", async ({
  page,
}) => {
  await page.goto("/");
  await page.locator('[data-testid=session-row][data-session-id="c1a70000-0000-4000-8000-0000000000c1"]').click();
  await pick(page, "connect-button");
  await expect(state(page)).toContainText("空闲");
  // The recording (clay-new) answers commands in its order: a prompt first, then `/cost`.
  const input = page.getByTestId("connect-input");
  await input.fill("/release v1");
  await input.press("Enter");
  await expect.poll(() => lastSent(page, "prompt")).toMatchObject({ text: "/release v1" });
  await expect(state(page)).toContainText("空闲");
  await input.fill("/cost");
  await input.press("Enter");
  const screen = page.getByTestId("screen-card");
  await expect(screen).toContainText("Total cost");
  await expect(screen).not.toContainText("Mac Client");

  // The mock Pane (src/features/terminal/bridge.ts) shows the same panel, at its own size.
  const [popup] = await Promise.all([page.waitForEvent("popup"), screen.getByTestId("screen-terminal").click()]);
  await expect(popup).toHaveTitle(/^终端 · /);
  const term = popup.getByTestId("terminal-screen");
  await expect(term).toContainText("Total cost");
  await expect(popup.getByTestId("terminal-size")).toHaveText("80×24");
  await expect(popup.getByTestId("terminal-back")).toHaveCount(0);

  // Typing goes to the Pane as it is (the mock Pane echoes it).
  await term.click();
  await popup.keyboard.type("zq9");
  await expect(term).toContainText("zq9");
  expect((await popup.evaluate(() => window.__termInput ?? [])).join("")).toBe("zq9");
  // Esc once; also when an input method is on, where WebKit gives it only a keyup.
  await popup.keyboard.press("Escape");
  await popup.evaluate(() =>
    document.querySelector("textarea")!.dispatchEvent(new KeyboardEvent("keyup", { key: "Escape", bubbles: true })),
  );
  await expect.poll(() => popup.evaluate(() => window.__termInput ?? [])).toEqual(["z", "q", "9", "\x1b", "\x1b"]);
  // An Esc that closes the input method's candidates is not sent.
  await popup.evaluate(() => {
    const t = document.querySelector("textarea")!;
    t.dispatchEvent(new CompositionEvent("compositionend", { data: "" }));
    t.dispatchEvent(new KeyboardEvent("keyup", { key: "Escape", bubbles: true }));
  });
  await popup.waitForTimeout(200);
  expect(await popup.evaluate(() => window.__termInput ?? [])).toEqual(["z", "q", "9", "\x1b", "\x1b"]);

  // Clicked again: the window already open, no second one.
  await screen.getByTestId("screen-terminal").click();
  await page.waitForTimeout(300);
  expect(page.context().pages()).toHaveLength(2);

  // Claude Code back at its input box: the window says so.
  await screen.getByTestId("screen-close").click();
  await expect(screen).toHaveCount(0);
  await expect(state(page)).toContainText("空闲");
  await expect(popup.getByTestId("terminal-back")).toHaveText("Claude Code 已回到输入框");

  // Disconnect: the Session runs on in its Pane, so the window stays (without the status).
  await pick(page, "connect-disconnect");
  await expect(popup.getByTestId("terminal-back")).toHaveCount(0);
  expect(popup.isClosed()).toBe(false);

  // 「···」 › 「显示 Terminal」, also without a connection: the window already open.
  await pick(page, "pane-terminal");
  await page.waitForTimeout(300);
  expect(page.context().pages()).toHaveLength(2);

  // The Pane closed (「关闭」 ends the Session and its Pane): the window closes.
  await pick(page, "connect-button");
  await expect(state(page)).toContainText("空闲");
  const closed = popup.waitForEvent("close");
  await page.getByTestId("connect-close").click();
  await closed;
});

test("「···」›「显示 Terminal」：没有 Connect 也能打开 clay 里 Session 的终端窗口", async ({ page }) => {
  await page.goto("/");
  await open(page, RICH);
  await inMenu(page, "pane-terminal", (b) => expect(b).toHaveText("显示 Terminal"));
  const [popup] = await Promise.all([page.waitForEvent("popup"), pick(page, "pane-terminal")]);
  await expect(popup.getByTestId("terminal-screen")).toContainText("Total cost");
  await expect(popup.getByTestId("terminal-back")).toHaveCount(0);
  // Not in clay (not in `ccrpc ls`): no such item.
  await open(page, NOT_LISTED);
  await page.getByTestId("header-more").click();
  await expect(page.getByTestId("header-menu").getByTestId("pane-terminal")).toHaveCount(0);
});

test("目录信任框是选择卡片", async ({ page }) => {
  await connectTo(page, TRUST);
  const card = page.getByTestId("choice-card");
  await expect(card).toContainText("trust");
  await expect(card.getByTestId("ai-badge")).toHaveCount(0);
  await card.getByTestId("choice-option").filter({ hasText: "Yes, I trust this folder" }).click();
  await expect(card).toHaveCount(0);
  await expect(state(page)).toContainText("空闲");
  expect(await lastSent(page, "answer_choice")).toMatchObject({ request_id: "choice:1", option_id: "1" });
});

test("识别中的界面：卡片写「正在识别界面…」，没学会时原地恢复，学会了换成选择卡片", async ({ page }) => {
  await connectTo(page, RICH);
  await expect(state(page)).toContainText("空闲");
  const dialog = (id: string, analyzing: boolean) => ({
    type: "dialog",
    request_id: id,
    waiting_for: "dialog open",
    lines: ["  Something new", "  1. Continue", "  2. Cancel"],
    analyzing,
  });
  const emit = (msg: unknown) => page.evaluate((m) => window.__cvCcrpc!.emit(m as Record<string, unknown>), msg);
  await emit({ type: "state", state: "waiting", waiting: { kind: "dialog", waiting_for: "dialog open" } });
  await emit(dialog("dialog:7", true));
  const card = page.getByTestId("screen-card");
  await expect(card).toContainText("正在识别界面…");
  await expect(card).toContainText("Something new");
  // Learned nothing: the same card, updated in place.
  await emit(dialog("dialog:7", false));
  await expect(card).toHaveCount(1);
  await expect(card).not.toContainText("正在识别界面…");
  await expect(card).toContainText("等待处理");

  // Learned: the dialog ends, its choice comes.
  await emit(dialog("dialog:8", true));
  await expect(card).toHaveCount(2);
  await emit({ type: "request_end", request_id: "dialog:8" });
  await emit({
    type: "choice",
    request_id: "choice:8",
    title: "Something new",
    options: [
      { id: "1", label: "Continue", meaning: "allow", description: null },
      { id: "2", label: "Cancel", meaning: "deny", description: null },
    ],
    tool_use_id: null,
    layout: "something-new",
    layout_source: "learned",
  });
  await expect(page.getByTestId("choice-card")).toBeVisible();
  await expect(card).toHaveCount(1);
  await emit({ type: "request_end", request_id: "dialog:7" });
  await expect(card).toHaveCount(0);
});
