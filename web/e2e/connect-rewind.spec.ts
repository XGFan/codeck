import { expect, test, type Page } from "@playwright/test";
import { disableAutoConnect, lastSent, open, pick, probe, state, RICH, NOT_LISTED, send } from "./connect-helpers";

// Connected by hand here (「Connect」 would race 自动 Connect); 自动 Connect has a test of its own.
test.beforeEach(({ page }) => disableAutoConnect(page));

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
        body: { kind: "userPrompt", text, images: [], origin: { kind: "human" }, relayed: [] },
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

test("回退：恢复代码与对话后原提示词进输入框、之后的对话淡化，切换 Session 仍在，新提示词进索引后复原；无代码改动时只恢复对话并保留草稿", async ({
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

  // Without code changes, only the conversation can be restored.
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

test("回退：Claude Code 记成 system 条目的本地命令（/context）没有「回退」按钮，/model 这类有", async ({ page }) => {
  await connectRewind(page);
  const buttons = page.getByTestId("transcript").getByTestId("rewind-button");
  await expect(buttons).toHaveCount(2);
  const command = (id: string, name: string, system: boolean) => ({
    id,
    timestampMs: Date.now(),
    hidden: false,
    inherited: false,
    body: {
      kind: "userPrompt",
      text: name,
      images: [],
      origin: { kind: "command", name, args: "", system },
      relayed: [],
    },
  });
  await page.evaluate(
    ({ sid, nodes }) => {
      for (const n of nodes) window.__cvMock!.appendNode(sid, n as never);
      window.__cvMock!.emit("sessionsChanged", { changed: [sid], removed: [], projectsChanged: false });
    },
    { sid: REDIS, nodes: [command("e2e-context", "/context", true), command("e2e-model", "/model", false)] },
  );
  await expect(promptRow(page, "e2e-context")).toBeVisible();
  await expect(promptRow(page, "e2e-model")).toBeVisible();
  await expect(promptRow(page, "e2e-context").getByTestId("rewind-button")).toHaveCount(0);
  await expect(promptRow(page, "e2e-context").getByTestId("fork-button")).toHaveCount(0);
  await expect(promptRow(page, "e2e-model").getByTestId("rewind-button")).toHaveCount(1);
  await expect(buttons).toHaveCount(3);
});
