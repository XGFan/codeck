import { expect, test, type Page } from "@playwright/test";
import { disableAutoConnect, lastSent, open, pick, probe, state, RICH } from "./connect-helpers";

// Connected by hand here (「Connect」 would race 自动 Connect); 自动 Connect has a test of its own.
test.beforeEach(({ page }) => disableAutoConnect(page));

const NEW_ID = "0e5e0000-0000-4000-8000-0000000000ee";

const openId = (page: Page) => page.evaluate(() => window.__cvStore!.getState().sessionId);

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

test("新建 Session：@ 补全相对它的目录——ccrpc 的 ready 早于进程文件、没有 cwd，cwd 随 started 到来", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByTestId("new-session").click();
  await page.getByTestId("new-session-other").click();
  await expect(state(page)).toContainText("空闲");
  await page.getByTestId("connect-input").fill("看看 @");
  await expect(page.getByTestId("path-option")).toHaveText(["src/"]);
});

test("新建 Session：发送提示词前关闭，页面回到未选中，不报找不到", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("new-session").click();
  await page.getByTestId("new-session-other").click();
  await expect(state(page)).toContainText("空闲");
  await page.getByTestId("connect-close").click();
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  await expect(page.getByTestId("transcript-empty")).toBeVisible();
  await expect(page.getByTestId("transcript-error")).toHaveCount(0);
  expect(await openId(page)).toBeNull();
});

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

test("clay 里未进索引的 Session：Connect 后不发提示词就关闭，页面回到未选中，不报找不到", async ({ page }) => {
  await page.goto("/");
  await page.locator('[data-testid=session-row][data-session-id="c1a70000-0000-4000-8000-0000000000c1"]').click();
  await expect(page.getByTestId("transcript-new")).toBeVisible();
  await pick(page, "connect-button");
  await expect(state(page)).toContainText("空闲");
  await page.getByTestId("connect-close").click();
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  await expect(page.getByTestId("transcript-empty")).toBeVisible();
  await expect(page.getByTestId("transcript-error")).toHaveCount(0);
});

test("新建 Session 失败：页面上写明原因，没有输入框，不显示「ccrpc 已退出」", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => ((window.__cvCcrpc as unknown as { newFails: boolean }).newFails = true));
  await page.getByTestId("new-session").click();
  await page.getByTestId("new-session-other").click();
  await expect(page.getByTestId("header").getByTestId("connect-error")).toHaveText(
    "新建 Session 失败：claude 没有启动",
  );
  await expect(page.getByTestId("connect-input")).toHaveCount(0);
  await expect(page.getByText("ccrpc 已退出")).toHaveCount(0);
  expect(await lastSent(page, "exit")).toBeUndefined();
});
