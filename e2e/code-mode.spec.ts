import { expect, test } from "@playwright/test";
import { connectTo, disableAutoConnect, lastSent, open, pick, state, TOOLS } from "./connect-helpers";
import { openSession, revealTranscript as reveal } from "./helpers";

const CODE_MODE_TITLE = "Code Mode：把重试上限改成 5";
const CODE_MODE_ID = "c0de5e55-0011-4000-8000-000000000011";

test("Code Mode：代码里调用的工具各占一行，代码只留一行浅色标记", async ({ page }) => {
  await openSession(page, CODE_MODE_TITLE);
  const code = (summary: string) => page.locator('[data-tool="REPL"]').filter({ hasText: summary });
  const tool = (name: string, text: string) => page.locator(`[data-tool="${name}"]`).filter({ hasText: text });

  // The code: what it is for, how long it is and ran; no verb, nothing nested in it.
  const run = code("把重试上限改成 5 并跑重试测试");
  await reveal(page, run);
  await expect(run).toContainText("4 行");
  await expect(run).toContainText("9 秒");
  await expect(run).not.toContainText("执行代码");
  await expect(run.locator("[data-tool]")).toHaveCount(0);
  // The calls it made: rows of their own, each opening as usual.
  const rg = tool("Bash", 'rg -l "max_attempts: 3" src');
  await expect(rg).toBeVisible();
  await expect(tool("Edit", "src/client/retry.rs")).toBeVisible();
  await rg.getByRole("button").first().click();
  await expect(rg.getByTestId("terminal")).toContainText("src/middleware/upstream.rs");
  // Opened: the code and what it returned.
  await run.getByRole("button").first().click();
  await expect(run.getByTestId("code-view")).toContainText('rg -l "max_attempts: 3" src');
  await expect(run.getByTestId("code-outcome")).toContainText("edited: [ 'src/client/retry.rs'");

  // The code threw: its line is red and says so; the error is where the result goes.
  const threw = code("读出配置里的重试参数");
  await reveal(page, threw);
  await expect(threw).toHaveAttribute("data-failed", "true");
  await expect(threw).toContainText("出错");
  await threw.getByRole("button").first().click();
  await expect(threw).toContainText("错误");
  await expect(threw.getByTestId("code-outcome")).toContainText("TypeError: Cannot read properties of undefined");

  // A call inside failed (the code went on): that call's row is red, not the code's.
  const partial = code("并行跑 clippy 与重试测试");
  await reveal(page, partial);
  await expect(partial).not.toHaveAttribute("data-failed", "true");
  await expect(tool("Bash", "cargo clippy")).toHaveAttribute("data-failed", "true");

  // Code that only asked: no line of its own; the question's card carries its code.
  const ask = page.getByTestId("ask").filter({ hasText: "跑哪些基准？" });
  await reveal(page, ask);
  await expect(ask.getByTestId("ask-status")).toContainText("已回答");
  await expect(ask.locator("[data-chosen]")).toHaveText(/只跑 retry/);
  await expect(code("问要跑哪些基准")).toHaveCount(0);
  await ask.getByTestId("ask-code").click();
  await expect(ask.getByTestId("code-view")).toContainText("tools.AskUserQuestion");

  // The call itself was rejected: no code ran, its result is the error.
  const refused = code("清掉构建缓存");
  await reveal(page, refused);
  await expect(refused).toHaveAttribute("data-failed", "true");
  await expect(refused).toContainText("出错");
  await expect(refused).not.toContainText("执行中");
  await refused.getByRole("button").first().click();
  await expect(refused.getByTestId("code-outcome")).toContainText("doesn't want to proceed");

  // Still running: nothing sent back yet.
  const running = code("跑重试路径的基准测试");
  await reveal(page, running);
  await expect(running).toContainText("执行中…");
});

test("Code Mode：按轮折叠只数代码里调用的工具", async ({ page }) => {
  await openSession(page, CODE_MODE_TITLE, { folded: true });
  // Three code runs made 4 + 1 + 2 calls; the runs themselves are not counted.
  await expect(page.getByTestId("turn-fold").first()).toContainText("7 次工具调用");
});

test("Code Mode：⌘F 找到代码里调用的工具的输出，打开那个调用", async ({ page }) => {
  await openSession(page, CODE_MODE_TITLE, { folded: true });
  await page.keyboard.press("Meta+f");
  await page.keyboard.type("port = 8080");
  const read = page.locator('[data-tool="Read"]').filter({ hasText: "config/default.toml" });
  await expect(read.getByTestId("code-view")).toContainText("port = 8080");
});

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

test.describe("Connect", () => {
  test.beforeEach(({ page }) => disableAutoConnect(page));

  test("Code Mode 里从代码发起的提问在输入区的卡片上作答", async ({ page }) => {
    await connectTo(page, TOOLS);
    await expect(state(page)).toContainText("空闲");
    // `tools.AskUserQuestion` in a REPL script: its call reaches the transcript only after the answer.
    await page.evaluate(() =>
      window.__cvCcrpc!.emit({
        type: "question",
        request_id: "repl_e2e_code",
        tool_use_id: "repl_e2e_code",
        agent_id: null,
        from_code: true,
        questions: [
          { question: "用哪个方案？", header: "方案", multiSelect: false, options: [{ label: "A" }, { label: "B" }] },
        ],
      }),
    );
    const q = page.getByTestId("question-card");
    await expect(q).toContainText("用哪个方案？");
    await expect(q).not.toContainText("Subagent");
    await expect(page.getByTestId("question-hint")).toHaveCount(0);
    await q.getByTestId("question-option").filter({ hasText: "B" }).click();
    await q.getByTestId("submit-answer").click();
    await expect
      .poll(() => lastSent(page, "answer_question"))
      .toMatchObject({ request_id: "repl_e2e_code", answers: [{ options: ["B"] }] });
  });

  test("Code Mode：代码运行期间它调用的工具由 ccrpc 报来，先在对话里各占一行；等待审批写在工具组上，提问答完才出现", async ({
    page,
  }) => {
    await page.goto("/");
    await page.evaluate(() => (window.__cvCcrpc as unknown as { list: (name: string) => void }).list("code-mode"));
    await open(page, CODE_MODE_TITLE);
    await pick(page, "connect-button");
    await expect(page.getByTestId("connect-panel")).toBeVisible();
    const transcript = page.getByTestId("transcript");
    const emit = (msg: Record<string, unknown>) => page.evaluate((m) => window.__cvCcrpc!.emit(m), msg);
    // The Session's last code (`cm-r4`) still runs: the transcript has none of its calls.
    const group = transcript.getByTestId("tool-group").last();
    await expect(group).toContainText("执行 2 段代码");
    await expect(transcript.getByTestId("group-current")).toHaveCount(0);
    const bench = { command: "cargo bench --bench retry" };
    await emit({ type: "code_call", tool_use_id: "repl_e2e_bench", name: "Bash", input: bench, code: "cm-r4" });
    // The call has a row of its own, grouped with its code: the group says what runs now.
    const current = transcript.getByTestId("group-current");
    await expect(current).toContainText("正在运行 cargo bench --bench retry…");
    // It asks for permission: the group says so (the card is in the input area).
    await emit({
      type: "permission_request",
      request_id: "repl_e2e_bench",
      tool_use_id: "repl_e2e_bench",
      tool_name: "Bash",
      input: bench,
      agent_id: null,
      from_code: true,
      options: [
        { id: "1", label: "Yes", meaning: "allow" },
        { id: "2", label: "No", meaning: "deny" },
      ],
      layout_source: "builtin",
    });
    await expect(current).toContainText("等待审批：运行 cargo bench --bench retry");
    await expect(page.getByTestId("permission-card")).toBeVisible();
    // Allowed and done: its output is what the hook reported, before the transcript has the call.
    await emit({ type: "request_end", request_id: "repl_e2e_bench" });
    await emit({ type: "state", state: "busy", waiting: null });
    await expect(page.getByTestId("permission-card")).toHaveCount(0);
    await emit({
      type: "code_call_end",
      tool_use_id: "repl_e2e_bench",
      is_error: false,
      response: { stdout: "retry/backoff  1.21 ms", stderr: "" },
    });
    await expect(current).toHaveCount(0);
    await expect(group).toContainText("运行 1 条命令、执行 1 段代码");
    await group.getByRole("button").first().click();
    const row = transcript.locator('[data-tool="Bash"]').filter({ hasText: "cargo bench --bench retry" });
    await row.getByRole("button").first().click();
    await expect(row.getByTestId("terminal")).toContainText("retry/backoff  1.21 ms");

    // A question from the code: answered on its card in the input area, in the transcript once answered.
    const question = {
      question: "再跑一遍吗？",
      header: "基准",
      multiSelect: false,
      options: [{ label: "跑" }, { label: "不跑" }],
    };
    await emit({
      type: "code_call",
      tool_use_id: "repl_e2e_ask",
      name: "AskUserQuestion",
      input: { questions: [question] },
      code: "cm-r4",
    });
    await emit({
      type: "question",
      request_id: "repl_e2e_ask",
      tool_use_id: "repl_e2e_ask",
      agent_id: null,
      from_code: true,
      questions: [question],
    });
    await expect(page.getByTestId("question-card")).toContainText("再跑一遍吗？");
    await expect(transcript.getByTestId("ask").filter({ hasText: "再跑一遍吗？" })).toHaveCount(0);
    await emit({ type: "request_end", request_id: "repl_e2e_ask" });
    await emit({ type: "state", state: "busy", waiting: null });
    await emit({
      type: "code_call_end",
      tool_use_id: "repl_e2e_ask",
      is_error: false,
      response: { questions: [question], answers: { "再跑一遍吗？": "不跑" } },
    });
    const ask = transcript.getByTestId("ask").filter({ hasText: "再跑一遍吗？" });
    await expect(ask.getByTestId("ask-status")).toContainText("已回答");
    await expect(ask.locator("[data-chosen]")).toHaveText(/不跑/);
  });
});
