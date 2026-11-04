// The current UI, screen by screen, light and dark, for the 「现状」 pages of the CoDeck design canvas.
// Run with `pnpm design:shots`; writes target/design-shots/<id>-{light,dark}.png and manifest.json
// (page, id, title per shot, in order). Steps follow the E2E specs; mock data and the recorded
// drive fixtures make every run the same.
import { expect, test, type Locator, type Page } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const OUT = join(import.meta.dirname, "../../target/design-shots");

const PAGES = [
  { id: "now-main", name: "现状 · 主界面" },
  { id: "now-drive", name: "现状 · 驾驶" },
  { id: "now-search-stats", name: "现状 · 搜索与统计" },
  { id: "now-settings", name: "现状 · 设置" },
] as const;
type PageId = (typeof PAGES)[number]["id"];

const shots: { page: PageId; id: string; title: string }[] = [];

const RICH = "为 /v1/chat 接口增加令牌桶限流中间件";
const TOOLS = "为 Flux 增加 kestrel-cli 镜像自动更新";
const TRUST = "/review 订单列表页";
const CLAY_NEW = "c1a70000-0000-4000-8000-0000000000c1";

/** Light, then dark, of the page as it is now. */
async function shot(page: Page, pageId: PageId, id: string, title: string) {
  await page.waitForTimeout(400);
  for (const scheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    await page.waitForTimeout(250);
    await page.screenshot({ path: join(OUT, `${id}-${scheme}.png`) });
  }
  await page.emulateMedia({ colorScheme: "light" });
  shots.push({ page: pageId, id, title });
}

async function open(page: Page, title: string) {
  await page.goto("/");
  await page.getByTestId("session-row").filter({ hasText: title }).click();
  await expect(page.getByTestId("header").getByRole("heading", { name: title })).toBeVisible();
}

async function drive(page: Page, title: string) {
  await open(page, title);
  await page.getByTestId("drive-connect").click();
  await expect(page.getByTestId("drive-panel")).toBeVisible();
}

async function send(page: Page, text: string) {
  await page.getByTestId("drive-input").fill(text);
  await page.getByTestId("drive-input").press("Enter");
}

const scrollBy = (scroller: Locator, dy: number) =>
  scroller.evaluate(
    (e, dy) =>
      new Promise<boolean>((done) => {
        const onScroll = () => requestAnimationFrame(() => done(true));
        e.addEventListener("scroll", onScroll, { once: true });
        const before = e.scrollTop;
        e.scrollTop = before + dy;
        if (e.scrollTop === before) {
          e.removeEventListener("scroll", onScroll);
          done(false);
        }
      }),
    dy,
  );

/** Scrolls the virtualized transcript upward from the bottom until `target` is rendered. */
async function reveal(page: Page, target: Locator) {
  const scroller = page.getByTestId("transcript");
  await expect(scroller.locator("[data-row]").first()).toBeVisible();
  await scrollBy(scroller, 1e7);
  for (let i = 0; i < 40 && (await target.count()) === 0; i++) {
    if (!(await scrollBy(scroller, -300))) break;
  }
  await target.first().scrollIntoViewIfNeeded();
}

test.beforeAll(() => mkdirSync(OUT, { recursive: true }));

test.afterAll(() => {
  writeFileSync(join(OUT, "manifest.json"), JSON.stringify({ pages: PAGES, shots }, null, 2) + "\n");
});

test("主界面", async ({ page }) => {
  await open(page, RICH);
  await shot(page, "now-main", "main-transcript", "Session 列表 · 对话 · 头部");

  await page.getByTestId("transcript").click({ position: { x: 4, y: 4 } });
  await page.keyboard.press("Meta+F");
  await page.getByTestId("find-input").fill("令牌桶");
  await shot(page, "now-main", "main-find", "⌘F 会话内查找");
  await page.keyboard.press("Escape");

  const card = page.locator('[data-testid="subagent-card"][data-agent-id="ag-sync1"]');
  await reveal(page, card);
  await shot(page, "now-main", "main-subagent-card", "Subagent 卡片与工具调用");
  await card.getByTestId("open-subagent").click();
  await expect(page.getByTestId("subagent-panel")).toBeVisible();
  await shot(page, "now-main", "main-subagent-panel", "Subagent 面板（Session 列表让出宽度）");
  await page.keyboard.press("Escape");

  await page.keyboard.press("Control+Meta+L");
  await shot(page, "now-main", "main-collapsed", "Session 列表收起（⌃⌘L）");
  await page.keyboard.press("Control+Meta+L");

  await page.locator(`[data-testid=session-row][data-session-id="${CLAY_NEW}"]`).click();
  await expect(page.getByTestId("transcript-new")).toBeVisible();
  await shot(page, "now-main", "main-new-session", "clay 里刚打开、未进索引的 Session（占位行）");
});

test("驾驶", async ({ page }) => {
  await drive(page, TOOLS);
  await expect(page.getByTestId("drive-state")).toContainText("空闲");
  await shot(page, "now-drive", "drive-idle", "已连接 · 空闲");

  await page.getByTestId("drive-input").fill("/");
  await expect(page.getByTestId("slash-menu")).toBeVisible();
  await shot(page, "now-drive", "drive-slash", "输入 / 的命令补全");
  await page.getByTestId("drive-input").fill("");

  await send(page, "MOCK-TOOL please");
  await expect(page.getByTestId("permission-card")).toBeVisible();
  await shot(page, "now-drive", "drive-permission", "审批卡片");
  await page.getByTestId("permission-card").getByTestId("allow").click();
  await expect(page.getByTestId("permission-card")).toHaveCount(0);

  await send(page, "MOCK-TOOL again");
  await page.getByTestId("permission-card").getByTestId("deny-with-message").click();
  await page.getByTestId("deny-message").fill("not now");
  await shot(page, "now-drive", "drive-deny-message", "拒绝并告诉 Claude");
  await page.getByTestId("deny-message").press("Enter");
  await expect(page.getByTestId("permission-card")).toHaveCount(0);

  await send(page, "MOCK-ASK which");
  await expect(page.getByTestId("question-hint")).toBeVisible();
  await page.evaluate(() => {
    const sid = "a7c63e92-0008-4000-8000-000000000008";
    const input = { questions: [{ question: "Which color?", header: "Color", multiSelect: false, options: [{ label: "Red", description: "warm" }, { label: "Blue", description: "cool" }] }] };
    window.__cvMock!.appendNode(sid, {
      id: "shot-ask",
      timestampMs: Date.now(),
      hidden: false,
      inherited: false,
      body: {
        kind: "assistant",
        messageId: "m-shot-ask",
        model: null,
        usage: null,
        isApiError: false,
        blocks: [{ kind: "toolCall", toolUseId: "toolu_mock0002", name: "AskUserQuestion", inputJson: JSON.stringify(input), inputTruncated: false, result: null, subagentId: null, workflowRunId: null, notificationNodeId: null }],
      },
    });
    window.__cvMock!.emit("sessionsChanged", { changed: [sid], removed: [], projectsChanged: false });
  });
  await page.getByTestId("question-jump").click();
  await expect(page.locator("[data-testid=ask][data-answering]")).toBeVisible();
  await shot(page, "now-drive", "drive-question", "提问：在对话里的卡片上作答");
});

test("驾驶 · 忙时与排队", async ({ page }) => {
  await drive(page, RICH);
  await expect(page.getByTestId("drive-state")).toContainText("空闲");
  await send(page, "改一下 README");
  await expect(page.getByTestId("drive-state")).toContainText("工作中");
  await send(page, "做完再跑一下测试");
  await expect(page.getByTestId("queue-item")).toHaveText(["做完再跑一下测试"]);
  await shot(page, "now-drive", "drive-busy-queue", "工作中 · claude 自己的排队与立即发送");
});

test("驾驶 · 选择卡片与面板", async ({ page }) => {
  await drive(page, TRUST);
  await expect(page.getByTestId("choice-card")).toBeVisible();
  await shot(page, "now-drive", "drive-choice", "选择卡片（目录信任框）");

  await page.goto("/");
  await page.locator(`[data-testid=session-row][data-session-id="${CLAY_NEW}"]`).click();
  await page.getByTestId("drive-connect").click();
  await expect(page.getByTestId("drive-state")).toContainText("空闲");
  await send(page, "/release v1");
  await expect(page.getByTestId("drive-input")).toHaveValue("");
  await expect(page.getByTestId("drive-state")).toContainText("空闲");
  await send(page, "/cost");
  await expect(page.getByTestId("screen-card")).toBeVisible();
  await shot(page, "now-drive", "drive-screen", "认不出的界面（/cost 面板）可关闭");

  // Ends right after connecting (as if `/exit` in the terminal): one line, 「驾驶」 back in the header.
  await drive(page, "（无标题）");
  await expect(page.getByTestId("session-exit")).toBeVisible();
  await expect(page.getByTestId("drive-connect")).toBeVisible();
  await shot(page, "now-drive", "drive-session-exit", "Session 在终端里退出后（可再次驾驶）");
});

test("搜索与统计", async ({ page }) => {
  await open(page, RICH);
  await page.keyboard.press("Meta+K");
  await expect(page.getByTestId("search")).toBeVisible();
  await shot(page, "now-search-stats", "search-empty", "⌘K 搜索 · 空输入");
  await page.getByTestId("search").getByRole("combobox").fill("限流");
  await page.waitForTimeout(600);
  await shot(page, "now-search-stats", "search-results", "⌘K 搜索 · 三段结果");
  await page.keyboard.press("Escape");

  await page.getByTestId("nav-stats").click();
  await expect(page.getByTestId("stats")).toBeVisible();
  await page.waitForTimeout(600);
  await shot(page, "now-search-stats", "stats", "统计");
});

test("设置", async ({ page }) => {
  await open(page, RICH);
  await page.keyboard.press("Meta+,");
  const dialog = page.getByRole("dialog");
  for (const [tab, id] of [
    ["通用", "settings-general"],
    ["数据", "settings-data"],
    ["驾驶", "settings-drive"],
    ["格式兼容性", "settings-diag"],
  ] as const) {
    await dialog.getByRole("tab", { name: tab }).click();
    await shot(page, "now-settings", id, `设置 · ${tab}`);
  }
});
