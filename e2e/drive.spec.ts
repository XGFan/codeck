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
// before `driveConnect` returns).

const RICH = "为 /v1/chat 接口增加令牌桶限流中间件";
const TOOLS = "为 Flux 增加 kestrel-cli 镜像自动更新";
const AUTH = "重构认证模块：拆分 token 校验与会话存储";
const JWT = "认证模块重构（改用 JWT 轮换）";
const NO_PLUGIN = "优化首页首屏加载（LCP 3.2s → 1.8s）";
const NOT_LISTED = "排查 Redis 连接池泄漏";
const TURNS = "迁移表单校验到 zod";
const TRUST = "/review 订单列表页";
const REFUSED = "Longhorn 卷扩容与备份策略";

type Probe = { sent: { msg: Record<string, unknown> }[]; lists: number; guiLog: string[]; badge: number };
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
  await expect(page.getByTestId("header")).toContainText("复制 resume 命令");
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
  const q = page.getByTestId("question-card");
  await expect(q).toContainText("Which color?");
  await q.getByTestId("question-option").filter({ hasText: "Blue" }).click();
  await q.getByTestId("submit-answer").click();
  await expect(page.getByTestId("turn-end")).toHaveAttribute("data-prompt-id", toolsTurns[2]);
  expect(await lastSent(page, "answer_question")).toMatchObject({ answers: [{ options: ["Blue"] }] });
});

test("忙时 Enter 排队（可撤销）、⌘Enter 插话；数字键作答；Dock 角标；AI 识别的选择；Esc 中止；ccrpc 退出后重连", async ({ page }) => {
  await drive(page, RICH);
  await expect(state(page)).toContainText("空闲");
  // At 1280 wide the title row keeps its buttons; the drive controls get a row of their own.
  const header = page.getByTestId("header");
  const title = await header.getByRole("heading", { name: RICH }).boundingBox();
  for (const b of [header.getByRole("button", { name: "复制 resume 命令" }), header.getByTestId("header-more")]) {
    const box = (await b.boundingBox())!;
    expect(Math.abs(box.y + box.height / 2 - (title!.y + title!.height / 2))).toBeLessThan(8);
  }
  expect((await page.getByTestId("drive-controls").boundingBox())!.y).toBeGreaterThan(title!.y + title!.height);
  // The LCP Live Session waits (CoDeck's Live data).
  await expect.poll(async () => (await probe(page)).badge).toBe(1);

  await send(page, "改一下 README");
  await expect(state(page)).toContainText("工作中");
  await expect(page.getByTestId("progress")).toContainText("Thinking");

  await send(page, "做完再跑一下测试");
  await expect(page.getByTestId("follow-ups")).toContainText("做完再跑一下测试");
  expect(await lastSent(page, "follow_up")).toMatchObject({ text: "做完再跑一下测试" });

  await send(page, "顺便把标题也改了", "Meta");
  expect(await lastSent(page, "steer")).toMatchObject({ text: "顺便把标题也改了" });

  const card = page.getByTestId("permission-card");
  await expect(card).toContainText("README.md");
  await expect(card).toContainText("新的一行");
  // A learned layout: the buttons carry the dialog's own words, not fixed ones.
  await expect(card.getByTestId("allow")).toHaveText("1. Yes");
  await expect(card.getByTestId("deny")).toHaveText("3. No");
  // This Session's card and the LCP Session, counted once each.
  await expect.poll(async () => (await probe(page)).badge).toBe(2);
  await page.getByTestId("cancel-follow-up").click();
  await expect(page.getByTestId("follow-ups")).toHaveCount(0);
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
  await expect(page.getByTestId("drive-input")).toBeDisabled();

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

  const controls = page.getByTestId("drive-controls");
  await controls.hover();
  await controls.getByRole("button", { name: "复制排查信息" }).click();
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

test("设置 › 驾驶：ccrpc 路径、doctor 的失败项、分析器与日志目录", async ({ page }) => {
  await page.goto("/");
  await page.keyboard.press("Meta+,");
  await page.getByRole("tab", { name: "驾驶" }).click();
  await expect(page.getByTestId("set-ccrpc")).toContainText("/mock/bin/ccrpc");
  await expect(page.getByTestId("set-ccrpc")).toContainText("CODECK_CCRPC");
  await expect(page.getByTestId("set-doctor").locator('[data-level="fail"]')).toContainText("ccrpc 插件未启用");
  await expect(page.getByTestId("set-analyzer")).toContainText("已开启");
  await expect(page.getByTestId("set-drive-logs")).toContainText("/mock/Logs/dev.joy.codeck/drive");
});

test("ccrpc ls 只在有 Live Session 或驾驶连接时轮询，恢复时立即查一次", async ({ page }) => {
  await page.goto("/");
  await open(page, RICH);
  await expect(page.getByTestId("drive-connect")).toBeVisible();
  const ids = await page.evaluate(() =>
    [...document.querySelectorAll("[data-testid=live-dot]")].map((d) => d.closest("[data-session-id]")!.getAttribute("data-session-id")!),
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
  await expect(page.getByTestId("drive-hint")).toHaveText("ccrpc 已退出");
  await expect(page.getByTestId("reconnect")).toBeVisible();
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

test("没发出的输入跟着 Session，切换后仍在", async ({ page }) => {
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
});

test("切回窗口时当前 Session 无事而别处在等作答：顶部一行提示，点击切过去", async ({ page }) => {
  await page.goto("/");
  // The LCP Live Session waits: no hint while it is the one on screen.
  await open(page, NO_PLUGIN);
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await page.waitForTimeout(200);
  await expect(page.getByTestId("elsewhere")).toHaveCount(0);

  await open(page, NOT_LISTED);
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await expect(page.getByTestId("elsewhere")).toHaveText(`${NO_PLUGIN} 在等你作答，点此切过去`);
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
