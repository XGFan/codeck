import { expect, test, type Page } from "@playwright/test";
import { openApp, openSession, revealTranscript as reveal } from "./helpers";
import { unfoldAll } from "./unfold";

const AGY_LIVE_TITLE = "agy：检查限流中间件的边界用例";
const AGY_LIVE_ID = "a6a10001-0012-4000-8000-000000000012";
const AGY_ENDED_TITLE = "agy：整理 Flux 部署清单说明";
const AGY_ENDED_ID = "a6a10002-0013-4000-8000-000000000013";
const FORK_ORIGIN_TITLE = "重构认证模块：拆分 token 校验与会话存储";
const WORKER_WEB_TITLE = "Agy Session Frontend And E2E";
const WORKER_LIVE_TITLE = "Agy Live And Haa Integration";
const WORKER_UNLINKED_TITLE = "Agy Standalone Log Audit";

async function openLiveSession(page: Page, title: string, { folded = false } = {}) {
  await page.goto("/");
  const row = page.getByTestId("live-row").filter({ hasText: title });
  await row.click();
  await expect(page.getByTestId("header").getByRole("heading", { name: title })).toBeVisible();
  if (!folded) await unfoldAll(page);
}

async function assertReadOnlyAgySession(page: Page, expectedId: string) {
  const header = page.getByTestId("header");
  await expect(header.getByTestId("session-cost")).toHaveText("Cost 未记录");
  await expect(header.getByTestId("resume-button")).toHaveCount(0);
  await expect(header.getByTestId("connect-close")).toHaveCount(0);
  await expect(header.getByTestId("connect-pending")).toHaveCount(0);
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  // Multi-turn agy Sessions also never offer per-prompt Fork.
  await expect(page.getByTestId("fork-button")).toHaveCount(0);

  await header.getByTestId("header-more").click();
  const menu = page.getByTestId("header-menu");
  await expect(menu).toBeVisible();
  await expect(menu.getByTestId("connect-button")).toHaveCount(0);
  await expect(menu.getByTestId("connect-disconnect")).toHaveCount(0);
  await expect(menu.getByTestId("pane-visibility")).toHaveCount(0);
  await expect(menu.getByTestId("pane-terminal")).toHaveCount(0);
  await expect(menu.getByTestId("fork-session")).toHaveCount(0);
  await expect(menu.getByRole("menuitemcheckbox", { name: "显示系统消息" })).toBeVisible();
  await expect(menu.getByRole("menuitem", { name: "复制 Session ID" })).toBeVisible();
  await expect(menu.getByRole("menuitem", { name: "在 Finder 中显示" })).toBeVisible();

  await menu.getByRole("menuitem", { name: "复制 Resume 命令" }).click();
  const copied = await page.evaluate(() => window.__cvCalls?.filter((c) => c.method === "copyText").pop()?.args[0]);
  expect(copied).toBe(`agy --conversation ${expectedId}`);
}

test("agy 译过的工具调用显示为命令 / 读文件 / 编辑行，展开详情看到原工具名与参数", async ({ page }) => {
  await openLiveSession(page, AGY_LIVE_TITLE);

  const read = page.locator('[data-tool="Read"]').filter({ hasText: "rate_limit.rs" }).first();
  await reveal(page, read);
  await expect(read).toContainText("读取");
  await read.getByRole("button").first().click();
  await expect(read.getByTestId("code-view").first()).toContainText("check_burst");
  const readOrig = read.getByTestId("agy-original");
  await expect(readOrig).toContainText("read_file");
  await expect(readOrig).toContainText('"StartLine": 1');

  const edit = page.locator('[data-tool="Edit"]').filter({ hasText: "rate_limit.rs" }).first();
  await reveal(page, edit);
  await expect(edit).toContainText("编辑");
  await edit.getByRole("button").first().click();
  await expect(edit.locator('[data-diff="add"]')).toContainText("InvalidBurst");
  const editOrig = edit.getByTestId("agy-original");
  await expect(editOrig).toContainText("edit_file");
  await expect(editOrig).toContainText('"Replacement"');

  const bash = page.locator('[data-tool="Bash"]').filter({ hasText: "cargo test -p lumen-api rate_limit" }).first();
  await reveal(page, bash);
  await expect(bash).toContainText("运行");
  await bash.getByRole("button").first().click();
  await expect(bash.getByTestId("terminal")).toContainText("6 passed");
  const bashOrig = bash.getByTestId("agy-original");
  await expect(bashOrig).toContainText("run_command");
  await expect(bashOrig).toContainText('"WaitMsBeforeAsync": 5000');

  // Untranslated agy tools render generically without an agy-original section.
  const generic = page.locator('[data-tool="manage_task"]').first();
  await reveal(page, generic);
  await generic.getByRole("button").first().click();
  await expect(generic.getByTestId("agy-original")).toHaveCount(0);
});

test("Live 与已结束的 agy Session 都没有 Connect / Resume / Fork 入口，保留复制 Resume 命令，Cost 写未记录", async ({
  page,
}) => {
  await openLiveSession(page, AGY_LIVE_TITLE);
  // Auto-connect is on by default: stay open longer than AUTO_CONNECT_DELAY_MS and verify no connection starts.
  await page.waitForTimeout(500);
  await assertReadOnlyAgySession(page, AGY_LIVE_ID);

  await openSession(page, AGY_ENDED_TITLE);
  await assertReadOnlyAgySession(page, AGY_ENDED_ID);
});

test("侧栏把已关联的 haa Worker 收进 owner，展开打开后头部可返回 owner", async ({ page }) => {
  await openApp(page);

  const ownerRow = page.getByTestId("session-row").filter({ hasText: FORK_ORIGIN_TITLE });
  const toggle = ownerRow.getByTestId("worker-toggle");
  await expect(toggle).toHaveText("2");
  await expect(toggle).toHaveAttribute("aria-expanded", "false");
  await expect(toggle).toHaveAttribute("aria-label", "显示 2 个 haa Worker");
  await expect(page.getByTestId("worker-row")).toHaveCount(0);

  // Clicking the Worker count expands its Workers without opening the owner Session.
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-expanded", "true");
  await expect(toggle).toHaveAttribute("aria-label", "收起 haa Worker");
  await expect(page.getByTestId("header")).toHaveCount(0);

  const workers = page.getByTestId("worker-row");
  await expect(workers).toHaveCount(2);
  await expect(workers.nth(0)).toContainText("agy-web");
  await expect(workers.nth(0)).toContainText(WORKER_WEB_TITLE);
  await expect(workers.nth(1)).toContainText("agy-live");
  await expect(workers.nth(1)).toContainText(WORKER_LIVE_TITLE);
  // Worker rows do not repeat the agy tag.
  await expect(workers.nth(0).getByTestId("agy-tag")).toHaveCount(0);

  await workers.nth(0).click();
  const header = page.getByTestId("header");
  await expect(header.getByRole("heading", { name: WORKER_WEB_TITLE })).toBeVisible();
  await expect(header).toContainText("agy · gemini-4-neon-high");
  const crumb = header.getByTestId("worker-crumb");
  await expect(crumb).toContainText(FORK_ORIGIN_TITLE);
  await expect(crumb).toContainText("haa Worker");
  await expect(crumb).toContainText("agy-web");

  // Clicking the owner link in the crumb navigates back to the owner Session.
  await crumb.getByRole("button", { name: FORK_ORIGIN_TITLE }).click();
  await expect(header.getByRole("heading", { name: FORK_ORIGIN_TITLE })).toBeVisible();
  await expect(header.getByTestId("worker-crumb")).toHaveCount(0);
});

test("Recent 不列 Worker，末尾可展开未关联的 haa Worker；非 Worker 的 agy 行带 agy 标记", async ({ page }) => {
  await openApp(page);

  // Non-worker agy Session has agy-tag in both Project and Recent.
  const agyProjectRow = page.getByTestId("session-row").filter({ hasText: AGY_ENDED_TITLE });
  await expect(agyProjectRow.getByTestId("agy-tag")).toHaveText("agy");
  const recent = page.getByTestId("recent");
  await expect(recent.getByTestId("recent-row").filter({ hasText: AGY_ENDED_TITLE }).getByTestId("agy-tag")).toHaveText(
    "agy",
  );
  // Recent does not list Worker rows directly.
  await expect(recent.getByTestId("worker-row")).toHaveCount(0);
  await expect(recent.getByTestId("recent-row").filter({ hasText: WORKER_WEB_TITLE })).toHaveCount(0);
  await expect(recent.getByTestId("recent-row").filter({ hasText: WORKER_UNLINKED_TITLE })).toHaveCount(0);

  // Scroll Recent to the end to reveal 「未关联的 haa Worker（1）」 and expand it.
  const recentList = recent.getByRole("listbox", { name: "Recent Session" });
  await recentList.evaluate((el) => (el.scrollTop = el.scrollHeight));
  const unlinkedToggle = recent.getByTestId("unlinked-toggle");
  await expect(unlinkedToggle).toContainText("未关联的 haa Worker（1）");
  await expect(unlinkedToggle).toHaveAttribute("aria-expanded", "false");

  await unlinkedToggle.click();
  await expect(unlinkedToggle).toHaveAttribute("aria-expanded", "true");
  await recentList.evaluate((el) => (el.scrollTop = el.scrollHeight));
  const unlinkedRow = recent.getByTestId("worker-row").filter({ hasText: WORKER_UNLINKED_TITLE });
  await expect(unlinkedRow).toBeVisible();
  await expect(unlinkedRow).toContainText("agy-solo");
  await expect(unlinkedRow).toContainText("homelab-infra");
  await expect(unlinkedRow.getByTestId("agy-tag")).toHaveCount(0);

  await unlinkedRow.click();
  const header = page.getByTestId("header");
  await expect(header.getByRole("heading", { name: WORKER_UNLINKED_TITLE })).toBeVisible();
  const crumb = header.getByTestId("worker-crumb");
  await expect(crumb).toContainText("haa Worker");
  await expect(crumb).toContainText("agy-solo");
  await expect(crumb.getByRole("button")).toHaveCount(0);
});

test("codex Worker 的 Task 卡片也能在右侧面板看完整过程（从 codex 的记录读，没有单独打开）", async ({ page }) => {
  await openSession(page, FORK_ORIGIN_TITLE, { folded: true });
  const card = page.getByTestId("haa-task-card").filter({ hasText: "codex-review" });
  await expect(card).toContainText("gpt-6.2-sol:high");
  await card.getByTestId("haa-task-open").click();
  await expect(card).toContainText("正在右侧查看");
  const panel = page.getByTestId("worker-panel");
  await expect(panel.getByTestId("panel-breadcrumb")).toContainText("codex-review");
  await expect(panel.getByTestId("panel-meta")).toContainText("codex · gpt-6.2-sol:high");
  await expect(panel.getByTestId("panel-meta")).toContainText("1 个 Task");
  await expect(panel.getByTestId("worker-panel-open")).toHaveCount(0);
  const transcript = panel.getByTestId("panel-transcript");
  await expect(transcript).toContainText("评审完成：没有阻塞问题，已提交 Result。");
  // A file change opens as a diff.
  await transcript.getByTestId("turn-fold").click();
  await transcript.getByText("HaaBar.tsx").first().click();
  await expect(transcript).toContainText("if (!open) return;");
});

test("设置 › 数据显示 agy 数据目录", async ({ page }) => {
  await openApp(page);
  await page.keyboard.press("Meta+,");
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("tab", { name: "数据" }).click();
  await expect(dialog.getByTestId("set-agy-root")).toContainText("agy 数据目录");
  await expect(dialog.getByTestId("set-agy-root-path")).toHaveText("/Users/dev/.gemini/antigravity-cli");
});

test("派活 Session 显示 haa Worker Task 卡片，右侧面板查看完整过程并可单独打开", async ({ page }) => {
  await openSession(page, FORK_ORIGIN_TITLE, { folded: true });

  const cards = page.getByTestId("haa-task-card");
  await expect(cards).toHaveCount(4);

  const webCard = cards.filter({ hasText: "agy-web" });
  const oldCard = cards.filter({ hasText: "agy-old" });
  const liveCard = cards.filter({ hasText: "agy-live" });

  await expect(webCard.getByTestId("haa-task-open")).toHaveText("查看完整过程 →");
  await expect(liveCard.getByTestId("haa-task-open")).toHaveText("查看完整过程 →");
  await expect(oldCard.getByTestId("haa-task-open")).toHaveCount(0);
  await expect(oldCard).toContainText("对话未关联");
  await expect(oldCard).toContainText("失败");

  // Click agy-web's open button -> right-hand Worker panel opens with agy-web's conversation.
  await webCard.getByTestId("haa-task-open").click();
  await expect(webCard).toContainText("正在右侧查看");
  const panel = page.getByTestId("worker-panel");
  await expect(panel).toBeVisible();
  await expect(panel.getByTestId("panel-breadcrumb")).toContainText("主对话");
  await expect(panel.getByTestId("panel-breadcrumb")).toContainText("agy-web");
  await expect(panel.getByTestId("panel-meta")).toContainText("agy · gemini-4-neon-high");
  await expect(panel.getByTestId("panel-meta")).toContainText("Cost 未记录");
  await expect(panel.getByTestId("panel-transcript")).toContainText(
    "已完成侧栏分组、头部返回行与 E2E 用例，所有测试通过。",
  );

  // Click 「单独打开」 -> switches to Worker's full-page view with worker-crumb, then crumb navigates back.
  await panel.getByTestId("worker-panel-open").click();
  await expect(panel).toHaveCount(0);
  const header = page.getByTestId("header");
  await expect(header.getByRole("heading", { name: WORKER_WEB_TITLE })).toBeVisible();
  const crumb = header.getByTestId("worker-crumb");
  await expect(crumb).toBeVisible();
  await crumb.getByRole("button", { name: FORK_ORIGIN_TITLE }).click();
  await expect(header.getByRole("heading", { name: FORK_ORIGIN_TITLE })).toBeVisible();
  await expect(header.getByTestId("worker-crumb")).toHaveCount(0);
});
