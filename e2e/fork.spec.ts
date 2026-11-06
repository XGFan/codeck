import { expect, test, type Page } from "@playwright/test";
import { openSession } from "./helpers";

// Fork against the mock ccrpc (src/ipc/mock-ccrpc.ts): `ccrpcFork` replays fork.jsonl (before a prompt), fork-session
// (the whole Session), fork-warning (restoring code: the rewind's ⚠ confirm card first) or fork-failed; each names
// the JWT Session of the mock data as the Fork. The Origin Session is the ended Redis one, not in clay.

const REDIS = "排查 Redis 连接池泄漏";
const REDIS_ID = "91d5be38-0004-4000-8000-000000000004";
const JWT = "认证模块重构（改用 JWT 轮换）";
const SECOND = "session.rs 第 44 行看起来没问题？";

type Probe = {
  forks: { sessionId: string; cwd: string; at: string | null; restore: string | null; title: string }[];
  sent: { msg: Record<string, unknown> }[];
  forkFails?: boolean;
};
const probe = (page: Page) => page.evaluate(() => window.__cvCcrpc as unknown as Probe);
const heading = (page: Page, title: string) => page.getByTestId("header").getByRole("heading", { name: title });

/** Opens the Fork menu under the Redis Session's second prompt. */
async function forkMenu(page: Page) {
  await page.getByTestId("user-bubble").filter({ hasText: SECOND }).hover();
  await page.getByTestId("prompt-meta").nth(1).getByTestId("fork-button").click();
  return page.getByTestId("fork-menu");
}

test("Fork：第一条提示词没有，之后的提示词有；只恢复对话后选中 Fork，输入框里是那条提示词", async ({ page }) => {
  await openSession(page, REDIS);
  const metas = page.getByTestId("prompt-meta");
  await expect(metas).toHaveCount(2);
  // An ended Session, not Connected: no 「回退」, but 「Fork」 from the second prompt on.
  await expect(metas.nth(0).getByTestId("fork-button")).toHaveCount(0);
  await expect(page.getByTestId("rewind-button")).toHaveCount(0);
  await expect(metas.nth(1).getByTestId("fork-button")).toHaveAttribute("title", "从这条提示词之前 Fork");
  const menu = await forkMenu(page);
  await expect(menu.getByTestId("fork-conversation")).toContainText("代码保持现状");
  await expect(menu.getByTestId("fork-both")).toContainText("Origin Session 也在用这些文件");
  await menu.getByTestId("fork-conversation").click();

  await expect(heading(page, JWT)).toBeVisible();
  const input = page.getByTestId("connect-input");
  await expect(input).toHaveValue(SECOND);
  await expect(input).toBeFocused();
  await expect(page.getByTestId("header").getByTestId("connect-close")).toBeVisible();
  expect((await probe(page)).forks).toEqual([
    {
      sessionId: REDIS_ID,
      cwd: "/Users/dev/Developer/lumen-api",
      at: "91d5-u1",
      restore: "conversation",
      title: `${REDIS} (Fork)`,
    },
  ]);
  // Done: nothing under way on the Origin Session.
  await page.getByTestId("session-row").filter({ hasText: REDIS }).click();
  await expect(heading(page, REDIS)).toBeVisible();
  await expect(page.getByTestId("fork-pending")).toHaveCount(0);
});

test("Fork：标题栏「···」里 Fork 整个 Session，输入框为空", async ({ page }) => {
  await openSession(page, REDIS);
  await page.getByTestId("header-more").click();
  await page.getByTestId("header-menu").getByTestId("fork-session").click();
  await expect(heading(page, JWT)).toBeVisible();
  await expect(page.getByTestId("connect-input")).toBeEnabled();
  await expect(page.getByTestId("connect-input")).toHaveValue("");
  expect((await probe(page)).forks).toMatchObject([{ sessionId: REDIS_ID, at: null, restore: null }]);
});

test("Fork 失败：Origin Session 的标题栏写明原因，什么都不留下", async ({ page }) => {
  await openSession(page, REDIS);
  await page.evaluate(() => {
    (window.__cvCcrpc as unknown as Probe).forkFails = true;
  });
  await (await forkMenu(page)).getByTestId("fork-conversation").click();
  await expect(page.getByTestId("header").getByTestId("connect-error")).toHaveText(
    "Fork 失败：终端的回退列表里没有这条提示词",
  );
  await expect(heading(page, REDIS)).toBeVisible();
  await expect(page.getByTestId("fork-pending")).toHaveCount(0);
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  await expect(page.getByTestId("prompt-meta").nth(1).getByTestId("fork-button")).toBeEnabled();
});

test("Fork 恢复代码时确认页带警告：切到 Fork 显示回退确认卡片，确认后完成", async ({ page }) => {
  await openSession(page, REDIS);
  await (await forkMenu(page)).getByTestId("fork-both").click();
  // The card waits on the Fork: shown on its Session before it is done.
  const card = page.getByTestId("rewind-card");
  await expect(card).toBeVisible();
  await expect(page.getByTestId("rewind-warning")).toBeVisible();
  await expect(card.getByTestId("rewind-code")).toHaveCount(0);
  // Back on the Origin Session: it is still forking, one Fork at a time.
  await page.getByTestId("session-row").filter({ hasText: REDIS }).click();
  await expect(heading(page, REDIS)).toBeVisible();
  await expect(page.getByTestId("fork-pending")).toHaveText("Fork 中…");
  const button = page.getByTestId("prompt-meta").nth(1).getByTestId("fork-button");
  await expect(button).toHaveText("Fork 中…");
  await expect(button).toBeDisabled();

  // 「Fork 中…」 leads back to the Fork, where its card waits.
  await page.getByTestId("fork-pending").click();
  await expect(heading(page, JWT)).toBeVisible();
  await page.getByTestId("rewind-card").getByTestId("rewind-both").click();
  expect((await probe(page)).sent.map((s) => s.msg).filter((m) => m.type === "answer_rewind")).toMatchObject([
    { request_id: "rewind:1", option: "both" },
  ]);
  await expect(page.getByTestId("rewind-card")).toHaveCount(0);
  await expect(page.getByTestId("connect-input")).toHaveValue(SECOND);
  await expect(page.getByTestId("rewound-divider")).toHaveCount(0);
});
