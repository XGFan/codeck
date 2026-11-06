import { expect, test, type Page } from "@playwright/test";
import { RICH_TITLE } from "./helpers";

const LUMEN = "/Users/dev/Developer/lumen-api";
const group = (page: Page, path: string) => page.locator(`[data-testid=sidebar-group][data-path="${path}"]`);
const rowTitles = (page: Page) => page.getByTestId("session-row").allTextContents();
const guiLog = (page: Page) => page.evaluate(() => (window.__cvCcrpc as unknown as { guiLog: string[] }).guiLog);
const setPickDir = (page: Page, dir: string | null) =>
  page.evaluate((d) => ((window.__cvCcrpc as unknown as { pickDir: string | null }).pickDir = d), dir);

async function groupMenu(page: Page, path: string, item: string) {
  const g = group(page, path);
  await g.hover();
  await g.getByRole("button", { name: /的更多操作$/ }).click();
  await page
    .getByRole("menuitem", { name: item })
    .or(page.getByRole("menuitemcheckbox", { name: item }))
    .click();
}

test("侧栏按 Project 分组：组按最后一次用户输入排序，Live Session 另列在「运行中」", async ({ page }) => {
  await page.goto("/");
  const groups = page.getByTestId("sidebar-group");
  await expect(groups).toHaveCount(3);
  await expect(groups.nth(0)).toHaveAttribute("data-path", LUMEN);
  await expect(groups.nth(1)).toContainText("orbit-web");
  await expect(groups.nth(2)).toContainText("homelab-infra");

  const running = page.getByTestId("running-row");
  // The Session open in clay but not indexed yet comes first, then by the latest user input.
  await expect(running).toHaveCount(3);
  await expect(running.nth(0)).toContainText("新 Session");
  await expect(running.nth(1)).toContainText(RICH_TITLE);
  await expect(running.nth(1)).toContainText("lumen-api");
  await expect(running.nth(2)).toContainText("优化首页首屏加载");
  // Each Session has one row in the groups, Live ones included.
  await expect(page.getByTestId("session-row").filter({ hasText: RICH_TITLE })).toHaveCount(1);

  await running.nth(2).click();
  await expect(page.getByTestId("header").getByRole("heading")).toContainText("优化首页首屏加载");
  await expect(page.getByTestId("session-row").filter({ hasText: "优化首页首屏加载" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
});

test("AI 回复不改变 Session 在组里的位置", async ({ page }) => {
  await page.goto("/");
  const lumen = page.getByTestId("session-row").filter({ hasText: "排查 Redis 连接池泄漏" });
  await expect(lumen).toBeVisible();
  const before = await rowTitles(page);
  const sid = (await lumen.getAttribute("data-session-id"))!;
  await page.evaluate((id) => {
    window.__cvMock!.appendNode(id, {
      id: "e2e-reply",
      timestampMs: Date.now(),
      hidden: false,
      inherited: false,
      body: {
        kind: "assistant",
        messageId: "m-e2e-reply",
        model: null,
        usage: null,
        isApiError: false,
        blocks: [{ kind: "text", text: "新的回复" }],
      },
    });
    window.__cvMock!.emit("sessionsChanged", { changed: [id], removed: [], projectsChanged: false });
  }, sid);
  // The list has refetched (the row's message count went up) and kept its order.
  await expect(page.getByTestId("session-row")).toHaveCount(before.length);
  await page.waitForTimeout(300);
  expect(await rowTitles(page)).toEqual(before);
});

test("组可收起，重启后保持；收起时显示最后时间与 Live 点", async ({ page }) => {
  await page.goto("/");
  const lumen = group(page, LUMEN);
  await lumen.getByRole("button", { name: /lumen-api/ }).click();
  await expect(page.getByTestId("session-row").filter({ hasText: "排查 Redis 连接池泄漏" })).toHaveCount(0);
  await expect(lumen.locator("[data-state=busy]")).toBeVisible();
  // Short enough that the row lands below the fold once the group opens.
  await page.setViewportSize({ width: 1200, height: 360 });
  await page.reload();
  await expect(group(page, LUMEN).getByRole("button", { name: /lumen-api/ })).toHaveAttribute("aria-expanded", "false");
  // Opening one of its Sessions from ⌘K opens the group again and scrolls to it.
  await page.keyboard.press("Meta+K");
  await page.getByTestId("search").getByRole("combobox").fill("Redis");
  await page.getByTestId("search-session").filter({ hasText: "排查 Redis 连接池泄漏" }).click();
  const redis = page.getByTestId("session-row").filter({ hasText: "排查 Redis 连接池泄漏" });
  await expect(redis).toHaveAttribute("aria-selected", "true");
  await expect(redis).toBeInViewport();
});

test("在组的「···」菜单里按 ↑↓ 只移动菜单项，不切换 Session", async ({ page }) => {
  await page.goto("/");
  const g = group(page, LUMEN);
  await g.hover();
  await g.getByRole("button", { name: /的更多操作$/ }).click();
  await expect(page.getByRole("menuitem", { name: "从侧栏移除" })).toBeVisible();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  await expect(page.locator("[data-testid=session-row][aria-selected=true]")).toHaveCount(0);
});

test("从侧栏移除后它的 Session 在「其他」里；「添加 Project」再加回来", async ({ page }) => {
  await page.goto("/");
  await groupMenu(page, "/Users/dev/Developer/homelab-infra", "从侧栏移除");
  await expect(page.getByTestId("sidebar-group")).toHaveCount(2);
  const longhorn = page.getByTestId("session-row").filter({ hasText: "Longhorn 卷扩容与备份策略" });
  await expect(longhorn).toHaveCount(0);
  await page.getByTestId("sidebar-other").click();
  await expect(longhorn).toContainText("homelab-infra");

  await page.getByTestId("add-project-button").click();
  const dialog = page.getByTestId("add-project");
  const rows = dialog.getByTestId("add-project-row");
  await expect(rows).toHaveCount(1);
  await rows.first().click();
  await dialog.getByRole("button", { name: "添加" }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByTestId("sidebar-group")).toHaveCount(3);
  await expect(page.getByTestId("sidebar-other")).toHaveCount(0);
});

test("收拢子 Project：路径下的 Project 成为子组，已单独添加的仍是自己的组", async ({ page }) => {
  await page.goto("/");
  await setPickDir(page, "/Users/dev/Developer");
  await page.getByTestId("add-project-button").click();
  const dialog = page.getByTestId("add-project");
  await dialog.getByRole("button", { name: "选择其他目录…" }).click();
  const row = dialog.getByTestId("add-project-row").filter({ hasText: "Developer" });
  await expect(row).toContainText("含 3 个子 Project");
  await dialog.getByRole("switch", { name: "收拢 Developer 的子 Project" }).check();
  await dialog.getByRole("button", { name: "添加" }).click();

  const dev = group(page, "/Users/dev/Developer");
  await expect(dev).toContainText("0");
  await groupMenu(page, "/Users/dev/Developer/orbit-web", "从侧栏移除");
  await expect(dev).toContainText("4");
  const sub = page.getByTestId("sidebar-sub").filter({ hasText: "orbit-web" });
  await expect(sub).toHaveAttribute("aria-expanded", "false");
  await sub.click();
  await expect(page.getByTestId("session-row").filter({ hasText: "迁移表单校验到 zod" })).toBeVisible();

  // Turned off in the group's menu: orbit-web's Sessions go to 「其他」.
  await groupMenu(page, "/Users/dev/Developer", "收拢子 Project（3 个）");
  await expect(page.getByTestId("sidebar-sub")).toHaveCount(0);
  await expect(page.getByTestId("sidebar-other")).toContainText("4");
});

test("⌘N 选已添加的 Project 新建 Session；组头「+」直接在它的目录新建", async ({ page }) => {
  await page.goto("/");
  await page.keyboard.press("Meta+N");
  const dialog = page.getByTestId("new-session-dialog");
  const targets = dialog.getByTestId("new-session-target");
  await expect(targets).toHaveCount(3);
  await expect(targets.first()).toContainText("lumen-api");
  await dialog.getByRole("combobox").fill("orbit");
  await expect(targets).toHaveCount(1);
  await page.keyboard.press("Enter");
  await expect(dialog).toBeHidden();
  await expect.poll(() => guiLog(page)).toContain("new /Users/dev/Developer/orbit-web");

  const homelab = group(page, "/Users/dev/Developer/homelab-infra");
  await homelab.hover();
  await homelab.getByRole("button", { name: "在 homelab-infra 新建 Session" }).click();
  await expect.poll(() => guiLog(page)).toContain("new /Users/dev/Developer/homelab-infra");
});

test("首次启动自动打开「添加 Project」；取消后侧栏为空，Session 都在「其他」里", async ({ page }) => {
  await page.goto("/?firstRun");
  const dialog = page.getByTestId("add-project");
  await expect(dialog).toBeVisible();
  await expect(dialog.getByTestId("add-project-row")).toHaveCount(3);
  await dialog.getByRole("button", { name: "取消" }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByTestId("sidebar-group")).toHaveCount(0);
  // The 10 Sessions and the one open in clay, not indexed yet.
  await expect(page.getByTestId("sidebar-other")).toContainText("11");
  await expect(page.getByTestId("running-row")).toHaveCount(3);
});
