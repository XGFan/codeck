import { expect, test, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
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
  TURNS,
  send,
} from "./connect-helpers";
import { composeEnd, composeStart } from "./ime";

// Connected by hand here (「Connect」 would race 自动 Connect); 自动 Connect has a test of its own.
test.beforeEach(({ page }) => disableAutoConnect(page));

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

/** The `uuid` ccrpc answered the n-th `prompt` of a recording with (the prompt's transcript entry). */
function recordedPromptUuid(name: string, n: number): string {
  return readFileSync(`src/mocks/data/connect/${name}.jsonl`, "utf8")
    .split("\n")
    .filter((l) => l.trim())
    .map((l) => JSON.parse(l))
    .filter((l) => l.dir === "recv" && l.msg?.type === "response" && l.msg.command === "prompt")[n]!.msg.data.uuid;
}

/** The input box's words, the image tokens left out. */
const words = (page: Page) =>
  page
    .getByTestId("connect-input")
    .evaluate((e: HTMLTextAreaElement) => e.value.replace(/\u2062[^\u2063]*\u2063/g, "|"));

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
        body: { kind: "userPrompt", text: "MOCK-TOOL please", images: [], origin: { kind: "human" }, relayed: [] },
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

test("回复前按 Esc 撤回的提示词：按 ccrpc 报的 uuid 从对话里拿掉、文字回到输入框，显示系统消息时标「已撤回」，再发同样的话照常显示", async ({
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
        body: { kind: "userPrompt", text: "改成两行\n的那个", images: [], origin: { kind: "human" }, relayed: [] },
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
      restored_uuid: "e2e-taken-back",
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
  await page.evaluate(() => window.__cvCcrpc!.emit({ type: "state", state: "busy", waiting: null }));
  await append("e2e-sent-again");
  await expect(bubble("e2e-sent-again")).toBeVisible();
  await expect(bubble("e2e-taken-back")).toHaveCount(0);
  // Taken back with no uuid said: nothing is hidden.
  await page.evaluate(() =>
    window.__cvCcrpc!.emit({
      type: "turn_end",
      prompt_id: null,
      trigger: "prompt",
      last_text: null,
      error: null,
      aborted: true,
      restored_text: "改成两行\n的那个",
      restored_uuid: null,
      background_tasks: [],
      duration_ms: 300,
      usage: { input_tokens: 0, output_tokens: 0, cache_read_input_tokens: 0, cache_creation_input_tokens: 0 },
      api_cost: 0,
    }),
  );
  await expect(bubble("e2e-sent-again")).toBeVisible();
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
  await expect
    .poll(() => lastSent(page, "prompt"))
    .toMatchObject({ text: "/add-dir ../infra-modules/", if_busy: "steer" });
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
  const controls = (await page.getByTestId("connect-controls").boundingBox())!;
  expect(controls.y + controls.height).toBeGreaterThan(box.y + box.height);
  const mode = (await page.getByTestId("connect-mode").boundingBox())!;
  const model = (await page.getByTestId("connect-model").boundingBox())!;
  const sendButton = (await page.getByRole("button", { name: "发送" }).boundingBox())!;
  expect(mode.x + mode.width).toBeLessThan(model.x);
  expect(model.x + model.width).toBeLessThan(sendButton.x);
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
  // An idle Teammate is neither counted nor listed, as in the TUI.
  await expect(tasks.filter({ hasText: "已经空闲的 Teammate" })).toHaveCount(0);
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
    .poll(() => lastSent(page, "prompt"))
    .toMatchObject({
      parts: [{ text: "做完再跑一下测试" }, { image: "/mock/Caches/dev.joy.codeck/images/1.png" }],
      if_busy: "steer",
    });
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
  await expect.poll(() => lastSent(page, "prompt")).toMatchObject({ text: "顺便把标题也改了", if_busy: "steer" });
  await expect.poll(async () => (await probe(page)).sent.filter((s) => s.msg.type === "send_now").length).toBe(2);

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
  expect(await lastSent(page, "answer_choice")).toMatchObject({ request_id: "choice:1", option_id: "2" });

  await send(page, "重写第一条");
  const screen = page.getByTestId("screen-card");
  await expect(screen).toContainText("Something new");
  await expect(screen.getByTestId("screen-terminal")).toHaveText("在终端窗口中处理");
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

test("斜杠命令：输入 / 列出补全，Tab 补全；一律走 prompt、不留占位气泡，打开的面板可关闭", async ({ page }) => {
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
  await expect.poll(() => lastSent(page, "prompt")).toMatchObject({ text: "/release-notes", if_busy: "steer" });
  await expect(input).toHaveValue("");
  // `delivered_as: "command"`: no prompt, no placeholder.
  await expect(page.getByTestId("pending-prompt")).toHaveCount(0);
  expect((await probe(page)).sent.some((s) => s.msg.type === "slash")).toBe(false);

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
  await expect.poll(() => lastSent(page, "prompt")).toMatchObject({ text: "/cost", if_busy: "steer" });
  await expect(page.getByTestId("pending-prompt")).toHaveCount(0);
  const screen = page.getByTestId("screen-card");
  await expect(screen).toContainText("Total cost");
  await screen.getByTestId("screen-close").click();
  expect(await lastSent(page, "abort")).toBeTruthy();
  await expect(screen).toHaveCount(0);
  await expect(state(page)).toContainText("空闲");

  // `/btw` answers in a panel, nothing in the transcript: its answer shows on a card of its own.
  await input.fill("/btw 这个端口是谁占的");
  await input.press("Enter");
  await expect.poll(() => lastSent(page, "prompt")).toMatchObject({ text: "/btw 这个端口是谁占的" });
  const btw = page.getByTestId("btw-card");
  await expect(btw).toContainText("/btw 这个端口是谁占的");
  await expect(btw).toContainText("已回答");
  await expect(btw.getByTestId("btw-answer")).toContainText("停掉它就能释放 1420");
  await expect(page.getByTestId("screen-card")).toHaveCount(0);
  await btw.getByTestId("btw-close").click();
  await expect(btw).toHaveCount(0);
  expect(await lastSent(page, "close_btw")).toBeTruthy();
  await expect(state(page)).toContainText("空闲");
});

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
      if_busy: "steer",
      id: expect.any(String),
    });
  await expect(page.getByTestId("image-chip")).toHaveCount(0);
});

test("显示工作中时这一轮其实刚结束：照样作为提示词发出，不报发送失败", async ({ page }) => {
  await connectTo(page, TURNS);
  await expect(state(page)).toContainText("空闲");
  // The app still shows the turn running; ccrpc finds the Session idle when the line comes.
  await page.evaluate(() => window.__cvCcrpc!.emit({ type: "state", state: "busy", waiting: null }));
  await expect(state(page)).toContainText("工作中");
  await send(page, "MOCK-SLOW please");
  await expect.poll(() => lastSent(page, "prompt")).toMatchObject({ text: "MOCK-SLOW please", if_busy: "steer" });
  expect(await lastSent(page, "steer")).toBeUndefined();
  await expect(page.getByTestId("connect-input")).toHaveValue("");
  await expect(page.getByText(/发送失败/)).toHaveCount(0);
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

test("排队：悬停可撤回的项，点「✕」发出 cancel_queue，该项从卡片里消失；别的 Session 发来的没有按钮", async ({
  page,
}) => {
  await connectTo(page, RICH);
  await expect(state(page)).toContainText("空闲");
  const item = (id: string, text: string, editable: boolean) => ({
    id,
    kind: editable ? "typed" : "session",
    from: editable ? null : "codeck-99",
    text,
    editable,
    images: [],
  });
  await page.evaluate(
    (items) => {
      window.__cvCcrpc!.emit({ type: "state", state: "busy", waiting: null });
      window.__cvCcrpc!.emit({ type: "queue", items });
    },
    [item("qa", "先把测试补上", true), item("qb", "合并前先拉一下 main。", false), item("qc", "再改文案", true)],
  );
  const card = page.getByTestId("queue");
  await expect(card.getByTestId("queue-item")).toHaveCount(3);
  // Only what was typed here can be taken back.
  await expect(card.getByTestId("queue-cancel")).toHaveCount(2);
  const cancel = card.getByTestId("queue-cancel").first();
  await expect(cancel).toHaveCSS("opacity", "0");
  await card.getByTestId("queue-item").first().hover();
  await expect(cancel).toHaveCSS("opacity", "1");
  await expect(cancel).toHaveAttribute("title", "撤回这条");
  await cancel.click();
  await expect.poll(() => lastSent(page, "cancel_queue")).toMatchObject({ queue_id: "qa" });
  await expect(card.getByTestId("queue-item")).toHaveText(["codeck-99：合并前先拉一下 main。", "；再改文案"]);
  await expect(card.getByTestId("queue-cancel")).toHaveCount(1);
  await expect(page.getByTestId("connect-error")).toHaveCount(0);

  // Refused: said above the input box, the item stays.
  await page.evaluate(() => (window.__cvCcrpc!.failNext = "cancel_queue"));
  await card.getByTestId("queue-item").last().hover();
  await card.getByTestId("queue-cancel").click();
  await expect(page.getByTestId("connect-error")).toContainText("撤回排队的消息失败");
  await expect(card.getByTestId("queue-item")).toHaveCount(2);
});
