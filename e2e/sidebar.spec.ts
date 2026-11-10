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

test("侧栏三块：Live 列出 Live Session，Project 按添加的顺序分组，Recent 列出其余的", async ({ page }) => {
  await page.goto("/");
  const groups = page.getByTestId("sidebar-group");
  await expect(groups).toHaveCount(3);
  await expect(groups.nth(0)).toHaveAttribute("data-path", LUMEN);
  await expect(groups.nth(1)).toContainText("orbit-web");
  await expect(groups.nth(2)).toContainText("homelab-infra");

  const live = page.getByTestId("live-row");
  // The Session open in clay but not indexed yet comes first, then by the latest user input.
  await expect(live).toHaveCount(3);
  await expect(live.nth(0)).toContainText("新 Session");
  await expect(live.nth(1)).toContainText(RICH_TITLE);
  await expect(live.nth(1)).toContainText("lumen-api");
  await expect(live.nth(2)).toContainText("优化首页首屏加载");
  // Waiting for an answer: tagged in 「Live」 and in its group.
  await expect(live.nth(2).getByTestId("waiting-tag")).toHaveText("等待作答");
  const waitingRow = page.getByTestId("session-row").filter({ hasText: "优化首页首屏加载" });
  await expect(waitingRow.getByTestId("waiting-tag")).toBeVisible();
  // One row per Session in the groups; 「Recent」 has the 8 that are not Live.
  await expect(page.getByTestId("session-row").filter({ hasText: RICH_TITLE })).toHaveCount(1);
  await expect(page.getByTestId("recent-toggle")).toContainText("8");
  await expect(page.getByTestId("recent-row").filter({ hasText: RICH_TITLE })).toHaveCount(0);
  await expect(page.getByTestId("recent-row").first()).toContainText("认证模块重构");

  await live.nth(2).click();
  await expect(page.getByTestId("header").getByRole("heading")).toContainText("优化首页首屏加载");
  await expect(waitingRow).toHaveAttribute("aria-selected", "true");
  await expect(live.nth(2)).toHaveAttribute("aria-selected", "true");
});

test("三块的标题、Live 点、组头箭头、Recent 行的标题左边对齐；Live 标题不带数量", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("recent-row").first()).toBeVisible();
  const lefts = await page.evaluate(() => {
    const pane = document.querySelector("[data-testid=pane-sessions]")!.getBoundingClientRect().left;
    // The left of the first piece of text inside `el`.
    const text = (el: Element) => {
      const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT, (n) =>
        n.textContent!.trim() ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_SKIP,
      );
      const r = document.createRange();
      r.selectNodeContents(walker.nextNode()!);
      return Math.round(r.getBoundingClientRect().left - pane);
    };
    const box = (el: Element) => Math.round(el.getBoundingClientRect().left - pane);
    const live = document.querySelector("[data-testid=live]")!;
    const project = [...document.querySelectorAll("[data-testid=pane-sessions] span")].find(
      (e) => e.textContent === "Project",
    )!;
    return {
      liveHeading: text(live.firstElementChild!),
      projectHeading: text(project),
      recentHeading: text(document.querySelector("[data-testid=recent-toggle]")!),
      liveDot: box(live.querySelector("[data-testid=live-dot]")!),
      groupChevron: box(document.querySelector("[data-testid=sidebar-group] svg")!),
      recentTitle: text(document.querySelector("[data-testid=recent-row]")!),
      liveHeadingText: live.firstElementChild!.textContent,
    };
  });
  expect(lefts).toEqual({
    liveHeading: 16,
    projectHeading: 16,
    recentHeading: 16,
    liveDot: 16,
    groupChevron: 16,
    recentTitle: 16,
    liveHeadingText: "Live",
  });
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

test("从侧栏移除后它的 Session 仍在 Recent 里；「添加 Project」再加回来", async ({ page }) => {
  await page.goto("/");
  await groupMenu(page, "/Users/dev/Developer/homelab-infra", "从侧栏移除");
  await expect(page.getByTestId("sidebar-group")).toHaveCount(2);
  await expect(page.getByTestId("session-row").filter({ hasText: "Longhorn 卷扩容与备份策略" })).toHaveCount(0);
  await expect(page.getByTestId("recent-row").filter({ hasText: "Longhorn 卷扩容与备份策略" })).toContainText(
    "homelab-infra",
  );

  await page.getByTestId("add-project-button").click();
  const dialog = page.getByTestId("add-project");
  const rows = dialog.getByTestId("add-project-row");
  await expect(rows).toHaveCount(1);
  await rows.first().click();
  await dialog.getByRole("button", { name: "添加" }).click();
  await expect(dialog).toBeHidden();
  // Added at the end.
  await expect(page.getByTestId("sidebar-group").nth(2)).toHaveAttribute(
    "data-path",
    "/Users/dev/Developer/homelab-infra",
  );
});

test("拖动组头调整 Project 的顺序；菜单里的「上移 / 下移」也可以", async ({ page }) => {
  await page.goto("/");
  const groups = page.getByTestId("sidebar-group");
  const paths = () => groups.evaluateAll((els) => els.map((e) => e.getAttribute("data-path")!.split("/").pop()));
  await expect.poll(paths).toEqual(["lumen-api", "orbit-web", "homelab-infra"]);
  // Folded, so every header is in view.
  await group(page, LUMEN)
    .getByRole("button", { name: /lumen-api/ })
    .click();
  await group(page, "/Users/dev/Developer/orbit-web")
    .getByRole("button", { name: /orbit-web/ })
    .click();

  const from = (await group(page, "/Users/dev/Developer/homelab-infra").boundingBox())!;
  const to = (await group(page, LUMEN).boundingBox())!;
  await page.mouse.move(from.x + 60, from.y + from.height / 2);
  await page.mouse.down();
  await page.mouse.move(from.x + 60, from.y + from.height / 2 - 10, { steps: 3 });
  // Dragging: the groups show their headers alone (homelab-infra's rows go too), a line marks where it lands.
  await expect(page.getByTestId("session-row")).toHaveCount(0);
  await page.mouse.move(to.x + 60, to.y + 4, { steps: 5 });
  await expect(page.getByTestId("drop-line")).toBeVisible();
  await page.mouse.up();
  // WebKit recomputes :hover only when the pointer moves, not when the rows move under it.
  await page.mouse.move(0, 0);
  await expect.poll(paths).toEqual(["homelab-infra", "lumen-api", "orbit-web"]);
  // The release does not fold the dragged group.
  await expect(group(page, "/Users/dev/Developer/homelab-infra").getByRole("button").first()).toHaveAttribute(
    "aria-expanded",
    "true",
  );

  await groupMenu(page, LUMEN, "下移");
  await expect.poll(paths).toEqual(["homelab-infra", "orbit-web", "lumen-api"]);
  await groupMenu(page, LUMEN, "上移");
  await expect.poll(paths).toEqual(["homelab-infra", "lumen-api", "orbit-web"]);
});

test("拖动时上面的组展开着：被按住的组头留在原处，手抖不改变顺序", async ({ page }) => {
  await page.goto("/");
  const groups = page.getByTestId("sidebar-group");
  const paths = () => groups.evaluateAll((els) => els.map((e) => e.getAttribute("data-path")!.split("/").pop()));
  const orbit = group(page, "/Users/dev/Developer/orbit-web");
  // lumen-api above it is open, its rows push orbit-web down.
  await expect(page.getByTestId("session-row").filter({ hasText: "排查 Redis 连接池泄漏" })).toBeVisible();
  const box = (await orbit.boundingBox())!;
  const x = box.x + 60;
  const y = box.y + box.height / 2;

  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x, y + 5, { steps: 2 });
  await expect(page.getByTestId("drop-line")).toBeVisible();
  // Folded under the pointer: the pressed header is where it was.
  expect(Math.abs((await orbit.boundingBox())!.y - box.y)).toBeLessThan(2);
  await page.mouse.up();
  await page.mouse.move(0, 0);
  await expect.poll(paths).toEqual(["lumen-api", "orbit-web", "homelab-infra"]);

  // One header up: before lumen-api.
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x, y - 30, { steps: 4 });
  await page.mouse.up();
  await page.mouse.move(0, 0);
  await expect.poll(paths).toEqual(["orbit-web", "lumen-api", "homelab-infra"]);
});

test("侧栏宽度：拖动右边缘调节（240–440），重启后保持，双击恢复 320", async ({ page }) => {
  await page.goto("/");
  const pane = page.getByTestId("pane-sessions");
  const width = async () => Math.round((await pane.boundingBox())!.width);
  expect(await width()).toBe(320);
  const h = (await page.getByTestId("sidebar-resize").boundingBox())!;
  await page.mouse.move(h.x + h.width / 2, h.y + 300);
  await page.mouse.down();
  await page.mouse.move(h.x + h.width / 2 + 60, h.y + 300, { steps: 4 });
  await page.mouse.up();
  expect(await width()).toBe(380);
  // Not past the limits.
  await page.mouse.move(h.x + 60 + h.width / 2, h.y + 300);
  await page.mouse.down();
  await page.mouse.move(h.x + 400, h.y + 300, { steps: 4 });
  await page.mouse.up();
  expect(await width()).toBe(440);
  await page.reload();
  expect(await width()).toBe(440);
  await page.getByTestId("sidebar-resize").dblclick();
  expect(await width()).toBe(320);
});

test("Recent 高度：拖动上边缘按整行调节，最少 1 行，重启后保持", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/");
  const recent = page.getByTestId("recent");
  const rows = async () => Math.round(((await recent.boundingBox())!.height - 37) / 28);
  await expect(page.getByTestId("recent-row").first()).toBeVisible();
  // 8 Sessions, 6 shown at first.
  expect(await rows()).toBe(6);
  const drag = async (dy: number) => {
    const h = (await page.getByTestId("recent-resize").boundingBox())!;
    await page.mouse.move(h.x + 100, h.y + h.height / 2);
    await page.mouse.down();
    await page.mouse.move(h.x + 100, h.y + h.height / 2 + dy, { steps: 4 });
    await page.mouse.up();
  };
  await drag(-56);
  expect(await rows()).toBe(8);
  await drag(400);
  expect(await rows()).toBe(1);
  await page.reload();
  await expect(page.getByTestId("recent-row").first()).toBeVisible();
  expect(await rows()).toBe(1);
});

test("Project 标题和 Live、Recent 一样固定，不随列表滚走", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 640 });
  await page.goto("/");
  const list = page.getByRole("listbox", { name: "Session 列表" });
  const heading = page.getByTestId("project-heading");
  await expect(page.getByTestId("session-row").first()).toBeVisible();
  const before = (await heading.boundingBox())!.y;
  await list.evaluate((el) => (el.scrollTop = el.scrollHeight));
  await expect.poll(() => list.evaluate((el) => el.scrollTop)).toBeGreaterThan(0);
  await expect(heading).toBeInViewport();
  expect((await heading.boundingBox())!.y).toBe(before);
  await expect(heading.getByTestId("add-project-button")).toBeVisible();
});

test("Recent 可收起，重启后保持", async ({ page }) => {
  await page.goto("/");
  const toggle = page.getByTestId("recent-toggle");
  await expect(page.getByTestId("recent-row").first()).toBeVisible();
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-expanded", "false");
  await expect(page.getByTestId("recent-row")).toHaveCount(0);
  // Collapsed, the heading is centered in what is left: a 32px heading under the border.
  expect(Math.round((await page.getByTestId("recent").boundingBox())!.height)).toBe(33);
  expect(Math.round((await toggle.boundingBox())!.height)).toBe(32);
  await page.reload();
  await expect(page.getByTestId("recent-toggle")).toHaveAttribute("aria-expanded", "false");
  await expect(page.getByTestId("recent-row")).toHaveCount(0);
});

test("⌘K 按 Project 筛选时只替换中间的 Project，Live 与 Recent 保留", async ({ page }) => {
  await page.goto("/");
  await page.keyboard.press("Meta+K");
  await page.getByTestId("search").getByRole("combobox").fill("lumen-api");
  await page.getByTestId("search-project").filter({ hasText: "lumen-api" }).click();
  await expect(page.getByTestId("project-chip")).toContainText("lumen-api");
  await expect(page.getByTestId("sidebar-group")).toHaveCount(0);
  // Started in orbit-web but editing lumen-api: listed, tagged with where it started.
  await expect(page.getByTestId("session-row").filter({ hasText: "迁移表单校验到 zod" })).toContainText("orbit-web");
  await expect(page.getByTestId("live-row")).toHaveCount(3);
  await expect(page.getByTestId("recent-toggle")).toBeVisible();
  await page.getByRole("button", { name: "清除 Project 筛选" }).click();
  await expect(page.getByTestId("sidebar-group")).toHaveCount(3);
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

  // Right-click: a new Session in the sub-Project's own directory, and the sub-group stays as it was.
  await sub.click({ button: "right" });
  await page.getByRole("menuitem", { name: "新建 Session" }).click();
  await expect.poll(() => guiLog(page)).toContain("new /Users/dev/Developer/orbit-web");
  await expect(sub).toHaveAttribute("aria-expanded", "true");

  // Turned off in the group's menu: orbit-web's Sessions are in no group (still in 「Recent」).
  await groupMenu(page, "/Users/dev/Developer", "收拢子 Project（3 个）");
  await expect(page.getByTestId("sidebar-sub")).toHaveCount(0);
  await expect(page.getByTestId("session-row").filter({ hasText: "迁移表单校验到 zod" })).toHaveCount(0);
});

test("⌘N 选已添加的 Project 新建 Session；组头「+」与右键菜单直接在它的目录新建", async ({ page }) => {
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

  // Right-clicking the header: the same, from a menu.
  await group(page, LUMEN).click({ button: "right" });
  await page.getByRole("menuitem", { name: "新建 Session" }).click();
  await expect.poll(() => guiLog(page)).toContain(`new ${LUMEN}`);
});

test("首次启动自动打开「添加 Project」；取消后没有组，Session 仍在 Live 与 Recent 里", async ({ page }) => {
  await page.goto("/?firstRun");
  const dialog = page.getByTestId("add-project");
  await expect(dialog).toBeVisible();
  await expect(dialog.getByTestId("add-project-row")).toHaveCount(3);
  await dialog.getByRole("button", { name: "取消" }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByTestId("sidebar-group")).toHaveCount(0);
  await expect(page.getByTestId("live-row")).toHaveCount(3);
  await expect(page.getByTestId("recent-toggle")).toContainText("8");
});
