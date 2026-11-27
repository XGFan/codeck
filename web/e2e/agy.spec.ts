import { expect, test, type Page } from "@playwright/test";
import { openApp, openSession, revealTranscript as reveal } from "./helpers";

const AGY_LIVE_TITLE = "agy：检查限流中间件的边界用例";
const AGY_LIVE_ID = "a6a10001-0012-4000-8000-000000000012";
const AGY_ENDED_TITLE = "agy：整理 Flux 部署清单说明";
const AGY_ENDED_ID = "a6a10002-0013-4000-8000-000000000013";

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
  await openSession(page, AGY_LIVE_TITLE);

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
  await openSession(page, AGY_LIVE_TITLE);
  // Auto-connect is on by default: stay open longer than AUTO_CONNECT_DELAY_MS and verify no connection starts.
  await page.waitForTimeout(500);
  await assertReadOnlyAgySession(page, AGY_LIVE_ID);

  await openSession(page, AGY_ENDED_TITLE);
  await assertReadOnlyAgySession(page, AGY_ENDED_ID);
});

test("设置 › 数据显示 agy 数据目录", async ({ page }) => {
  await openApp(page);
  await page.keyboard.press("Meta+,");
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("tab", { name: "数据" }).click();
  await expect(dialog.getByTestId("set-agy-root")).toContainText("agy 数据目录");
  await expect(dialog.getByTestId("set-agy-root-path")).toHaveText("/Users/dev/.gemini/antigravity-cli");
});
