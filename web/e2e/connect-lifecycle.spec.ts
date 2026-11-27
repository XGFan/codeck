import { expect, test, type Page } from "@playwright/test";
import {
  connectTo,
  disableAutoConnect,
  inMenu,
  lastSent,
  open,
  pick,
  probe,
  state,
  TOOLS,
  RICH,
  NOT_LISTED,
  TURNS,
  send,
} from "./connect-helpers";

// Connected by hand here (「Connect」 would race 自动 Connect); 自动 Connect has a test of its own.
test.beforeEach(({ page }) => disableAutoConnect(page));

const AUTH = "重构认证模块：拆分 token 校验与会话存储";

const JWT = "认证模块重构（改用 JWT 轮换）";

const NO_PLUGIN = "优化首页首屏加载（LCP 3.2s → 1.8s）";

const REFUSED = "Longhorn 卷扩容与备份策略";

const renameItem = (page: Page) => page.getByRole("menuitem", { name: /^重命名/ });

const remoteConn = (page: Page, name: string) => page.evaluate((n) => window.__cvCcrpc!.remoteConn(n), name);

test("「···」菜单：ccrpc ls 里的 Live Session 可 Connect，插件未报到时禁用，其他 Session 没有；标题栏里没有", async ({
  page,
}) => {
  await page.goto("/");
  await open(page, RICH);
  await expect(page.getByTestId("header").getByRole("button", { name: "Connect" })).toHaveCount(0);
  await inMenu(page, "connect-button", (b) => expect(b).toBeEnabled());
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);

  await open(page, NO_PLUGIN);
  await inMenu(page, "connect-button", async (b) => {
    await expect(b).toHaveAttribute("data-disabled");
    await expect(b).toContainText("ccrpc 插件未加载，重启 claude 后可用");
  });

  await open(page, NOT_LISTED);
  await inMenu(page, "connect-button", (b) => expect(b).toHaveCount(0));
});

test("「···」菜单：clay 里的 Session 不论是否 Connect 都可在 ToH 隐藏 / 打开，不在 clay 里的没有", async ({ page }) => {
  await page.goto("/");
  await open(page, RICH);
  await inMenu(page, "pane-visibility", (b) => expect(b).toContainText("在 ToH 隐藏"));
  await pick(page, "pane-visibility");
  await inMenu(page, "pane-visibility", (b) => expect(b).toContainText("在 ToH 打开"));
  const [hide] = (await probe(page)).paneHides;
  expect(hide.hidden).toBe(true);

  await pick(page, "connect-button");
  await expect(page.getByTestId("connect-panel")).toBeVisible();
  await inMenu(page, "pane-visibility", (b) => expect(b).toContainText("在 ToH 打开"));
  await pick(page, "pane-visibility");
  await inMenu(page, "pane-visibility", (b) => expect(b).toContainText("在 ToH 隐藏"));
  expect((await probe(page)).paneHides).toEqual([hide, { holderId: hide.holderId, hidden: false }]);

  await open(page, NOT_LISTED);
  await inMenu(page, "pane-visibility", (b) => expect(b).toHaveCount(0));
});

test("Session 行右键「重命名」：未 Connect 时置灰，Connected 且空闲时发 rename；失败时留在对话框写原因", async ({
  page,
}) => {
  await page.goto("/");
  const row = page.locator('[data-testid="session-row"]').filter({ hasText: TOOLS }).first();
  await row.click({ button: "right" });
  await expect(renameItem(page)).toHaveAttribute("data-disabled", "");
  await expect(renameItem(page)).toContainText("Connect 后可用");
  await page.keyboard.press("Escape");

  await open(page, TOOLS);
  await pick(page, "connect-button");
  await expect(state(page)).toContainText("空闲");
  await row.click({ button: "right" });
  await renameItem(page).click();
  const dialog = page.getByTestId("rename-dialog");
  const input = dialog.getByTestId("rename-input");
  // The menu going away leaves the focus in the input, with the title selected: typing replaces it.
  await expect(input).toBeFocused();
  await expect(input).toHaveValue(TOOLS);
  await page.keyboard.type("第一次");
  await expect(input).toHaveValue("第一次");

  await page.evaluate(() => ((window.__cvCcrpc as unknown as { failNext: string }).failNext = "rename"));
  await input.press("Enter");
  await expect(dialog.getByTestId("rename-error")).toContainText("重命名失败");
  await expect(input).toHaveValue("第一次");

  await input.fill("  登录页\t重构 ");
  await input.press("Enter");
  await expect(dialog).toBeHidden();
  expect(await lastSent(page, "rename")).toMatchObject({ title: "登录页 重构" });

  // The window is usable after the dialog opened from a menu closed.
  expect(await page.evaluate(() => document.body.style.pointerEvents)).not.toBe("none");
  await row.click();
});

test("自动 Connect：打开可 Connect 的 Session 就连上，Disconnect 后不再自动连，再次打开才会；可在设置里关掉", async ({
  page,
}) => {
  await page.clock.install();
  // The default (on): once, not again on the reload below.
  await page.addInitScript(() => {
    if (sessionStorage.getItem("e2e-default")) return;
    sessionStorage.setItem("e2e-default", "1");
    localStorage.removeItem("codeck.autoConnect");
  });
  await page.goto("/");
  // Visited once, so its header comes from the cache when opened again below (the header is not remounted).
  await open(page, NOT_LISTED);
  // On by default.
  await open(page, RICH);
  await expect(page.getByTestId("connect-panel")).toBeVisible();
  await expect(state(page)).toContainText("空闲");
  await pick(page, "connect-disconnect");
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  await page.clock.runFor(800);
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  await inMenu(page, "connect-button", (b) => expect(b).toBeVisible());
  // Opened again: connected again.
  await open(page, NOT_LISTED);
  await open(page, RICH);
  await expect(page.getByTestId("connect-panel")).toBeVisible();
  // Opened while connected already (another Session connected meanwhile): a Disconnect stays.
  await open(page, TOOLS);
  await expect(page.getByTestId("connect-panel")).toBeVisible();
  await open(page, RICH);
  await pick(page, "connect-disconnect");
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  await page.clock.runFor(800);
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  // Not a Session whose plugin has not reported.
  await open(page, NO_PLUGIN);
  await page.clock.runFor(800);
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);

  // Off in Settings › 通用, and remembered.
  await page.getByTestId("settings-button").click();
  await page.getByRole("tab", { name: "通用" }).click();
  await expect(page.getByTestId("auto-connect-true")).toHaveAttribute("aria-checked", "true");
  await page.getByTestId("auto-connect-false").click();
  await page.keyboard.press("Escape");
  await open(page, TURNS);
  await page.clock.runFor(800);
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  await inMenu(page, "connect-button", (b) => expect(b).toBeEnabled());
  await page.reload();
  await page.getByTestId("settings-button").click();
  await page.getByRole("tab", { name: "通用" }).click();
  await expect(page.getByTestId("auto-connect-false")).toHaveAttribute("aria-checked", "true");
});

test("后台任务随 tasks 事件更新", async ({ page }) => {
  await connectTo(page, TOOLS);
  await expect(state(page)).toContainText("空闲");
  await expect(page.getByTestId("tasks-shell")).toHaveCount(0);
  const shell = (status: string) => ({
    id: "b-e2e",
    kind: "shell",
    tool_use_id: null,
    description: "pnpm dev",
    command: "pnpm dev",
    status,
    idle: false,
  });
  const tasks = (status: string) =>
    page.evaluate((t) => window.__cvCcrpc!.emit({ type: "tasks", tasks: [t] }), shell(status));
  await tasks("running");
  await expect(page.getByTestId("tasks-shell")).toHaveText("1 Shell");
  await tasks("completed");
  await expect(page.getByTestId("tasks-shell")).toHaveCount(0);
  // Not asked for: the list comes with the events.
  expect(await lastSent(page, "get_tasks")).toBeUndefined();
});

test("Pane 里换了 Session（/clear）时选中跟着切换；复制排查信息；Disconnect", async ({ page }) => {
  await connectTo(page, AUTH);
  await expect(state(page)).toContainText("空闲");
  await send(page, "/clear");
  await expect(page.getByTestId("header").getByRole("heading", { name: JWT })).toBeVisible();
  await expect(page.getByTestId("connect-panel")).toBeVisible();

  await page.getByTestId("header-more").click();
  await page.getByRole("menuitem", { name: "复制排查信息" }).click();
  const copied = await page.evaluate(() => window.__cvCalls!.filter((c) => c.method === "copyText").pop()?.args[0]);
  expect(copied).toMatch(
    /^holder_id: mclear0000001\nsession_id: c04e77a2-0003-4000-8000-000000000003\ncwd: \/work\/lumen\nlogs: \S+$/,
  );

  await pick(page, "connect-disconnect");
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  await expect(page.getByTestId("connect-controls")).toHaveCount(0);
});

test("关闭：空闲且没有后台任务时直接结束 Session 与 Pane，并 Disconnect", async ({ page }) => {
  await connectTo(page, TURNS);
  await expect(state(page)).toContainText("空闲");
  await page.getByTestId("connect-close").click();
  await expect(page.getByTestId("connect-close-confirm")).toHaveCount(0);
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  expect(await lastSent(page, "exit")).toMatchObject({ close_pane: true, stop_tasks: false });
  expect((await probe(page)).sent.some((s) => s.msg.type === "abort")).toBe(false);
  // The Pane is gone from `ccrpc ls`: nothing to connect.
  await inMenu(page, "connect-button", (b) => expect(b).toHaveCount(0));
  await inMenu(page, "connect-disconnect", (b) => expect(b).toHaveCount(0));
});

test("关闭：Claude Code 退出时说还有后台任务（CoDeck 不知道的，如它自己的 monitor），先确认再停掉", async ({
  page,
}) => {
  await connectTo(page, TURNS);
  await expect(state(page)).toContainText("空闲");
  await page.evaluate(() => ((window.__cvCcrpc as unknown as { monitorRunning: boolean }).monitorRunning = true));
  await page.getByTestId("connect-close").click();
  const confirm = page.getByTestId("connect-close-confirm");
  await expect(confirm).toContainText("将停掉后台任务，结束 Session 并关闭它的 Pane。");
  expect(await lastSent(page, "exit")).toMatchObject({ stop_tasks: false });
  await expect(page.getByTestId("connect-panel")).toBeVisible();
  await expect(page.getByTestId("connect-error")).toHaveCount(0);
  // Cancelled: nothing more is sent, and closing again asks again.
  await confirm.getByRole("button", { name: "取消" }).click();
  await expect(confirm).toHaveCount(0);
  await page.getByTestId("connect-close").click();
  await expect(confirm).toBeVisible();
  expect((await probe(page)).sent.filter((s) => s.msg.type === "exit")).toHaveLength(2);
  await page.getByTestId("connect-close-ok").click();
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  expect(await lastSent(page, "exit")).toMatchObject({ close_pane: true, stop_tasks: true });
});

test("关闭：还有后台任务时先确认，取消则什么也不发", async ({ page }) => {
  await connectTo(page, RICH);
  await expect(state(page)).toContainText("空闲");
  await page.getByTestId("connect-close").click();
  const confirm = page.getByTestId("connect-close-confirm");
  await expect(confirm).toContainText("停掉 4 个后台任务");
  await confirm.getByRole("button", { name: "取消" }).click();
  await expect(confirm).toHaveCount(0);
  expect(await lastSent(page, "exit")).toBeUndefined();
  await expect(page.getByTestId("connect-panel")).toBeVisible();

  await page.getByTestId("connect-close").click();
  await page.getByTestId("connect-close-ok").click();
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  expect(await lastSent(page, "exit")).toMatchObject({ close_pane: true, stop_tasks: true });
});

test("关闭：忙时确认后先中止本轮再结束", async ({ page }) => {
  await connectTo(page, RICH);
  await send(page, "改一下 README");
  await expect(state(page)).toContainText("工作中");
  await page.getByTestId("connect-close").click();
  await expect(page.getByTestId("connect-close-confirm")).toContainText("中止本轮、停掉 4 个后台任务");
  await page.getByTestId("connect-close-ok").click();
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  const types = (await probe(page)).sent.map((s) => s.msg.type);
  expect(types.slice(types.indexOf("abort"))).toEqual(["abort", "exit"]);
});

test("ccrpc ls --watch 只在有 Live Session 或连接时运行，恢复时立即重新开始", async ({ page }) => {
  await page.goto("/");
  await open(page, RICH);
  await inMenu(page, "connect-button", (b) => expect(b).toBeVisible());
  const ids = await page.evaluate(() =>
    // Indexed rows only: the clay-new placeholder is Live through `ccrpc ls`, not the mock data.
    [...document.querySelectorAll("[data-testid=live-row]:not([data-pending]) [data-testid=live-dot]")].map((d) =>
      d.closest("[data-session-id]")!.getAttribute("data-session-id")!,
    ),
  );
  expect(ids.length).toBeGreaterThan(0);
  await page.evaluate((list) => list.forEach((id) => window.__cvMock!.setLive(id, null)), ids);
  await expect(page.getByTestId("live-dot")).toHaveCount(0);
  // With nothing Live the watch stops and what it said last is dropped.
  await inMenu(page, "connect-button", (b) => expect(b).toHaveCount(0));
  await expect.poll(async () => (await probe(page)).watching).toBe(false);
  const before = (await probe(page)).watchStarts;
  // 「新建 Session」 works without an `ccrpc ls` answer.
  await expect(page.getByTestId("new-session")).toBeVisible();

  await page.evaluate(
    (id) => window.__cvMock!.setLive(id, { status: "idle", rawStatus: "idle", waitingFor: null, pid: 1 }),
    ids[0]!,
  );
  await expect.poll(async () => (await probe(page)).watchStarts, { timeout: 5000 }).toBe(before + 1);
  expect((await probe(page)).watching).toBe(true);
  await inMenu(page, "connect-button", (b) => expect(b).toBeVisible());
});

test("ccrpc 在连接命令返回前就退出：显示退出横幅而不是一直「连接中」", async ({ page }) => {
  await connectTo(page, REFUSED);
  await expect(page.getByTestId("proc-exit")).toContainText("退出码 1");
  // The reason is ccrpc's `fatal` message.
  await expect(page.getByTestId("proc-exit")).toContainText("Holder 版本太旧");
  // Only the banner says it: no input box under it.
  await expect(page.getByTestId("connect-input")).toHaveCount(0);
  await expect(page.getByTestId("reconnect")).toBeVisible();
});

test("Session 在终端里退出后：输入区只剩一行提示，「···」菜单里重新出现「Connect」，resume 后可再次 Connect", async ({
  page,
}) => {
  await connectTo(page, "（无标题）");
  await expect(page.getByTestId("session-exit")).toContainText("Session 已结束");
  // ccrpc exits by itself after `session_exit`: that is not a failure, the card stays.
  await expect(page.getByTestId("proc-exit")).toHaveCount(0);
  await expect(page.getByText("ccrpc 已退出")).toHaveCount(0);
  await expect(page.getByTestId("connect-input")).toHaveCount(0);
  await inMenu(page, "connect-disconnect", (b) => expect(b).toHaveCount(0));
  // Still in `ccrpc ls` (resumed in the terminal): no Resume, connecting again replaces the ended connection.
  await expect(page.getByTestId("session-exit-resume")).toHaveCount(0);
  await pick(page, "connect-button");
  await expect.poll(async () => (await probe(page)).guiLog.filter((l) => l === "connect mresumed00001").length).toBe(2);
  expect((await probe(page)).guiLog).toContain("close (replaced)");
  await expect(page.getByTestId("session-exit")).toHaveCount(1);
});

test("Resume：已结束、不在 clay 里的 Session 在新 Pane 里重新运行并 Connect", async ({ page }) => {
  const redis = "91d5be38-0004-4000-8000-000000000004";
  await page.goto("/");
  await open(page, "排查 Redis 连接池泄漏");
  const header = page.getByTestId("header");
  await inMenu(page, "connect-button", (b) => expect(b).toHaveCount(0));
  const resume = header.getByTestId("resume-button");
  await expect(resume).toHaveAttribute("title", /从 Main Line 末尾继续/);
  await resume.click();
  // The same Session, connected: no 「新 Session」 row, the header and the input area as for Connect.
  await expect(header.getByRole("heading", { name: "排查 Redis 连接池泄漏" })).toBeVisible();
  await expect(header.getByTestId("connect-close")).toBeVisible();
  await expect(page.getByTestId("connect-input")).toBeEnabled();
  await expect(header.getByTestId("resume-button")).toHaveCount(0);
  await expect(page.locator(`[data-testid=session-row][data-pending][data-session-id="${redis}"]`)).toHaveCount(0);
  await expect(page.locator(`[data-testid=session-row][data-session-id="${redis}"]`)).toHaveCount(1);
  expect((await probe(page)).guiLog).toContain(`resume ${redis}`);
});

test("Resume：ccrpc 在 ready 里就给出 Session ID 时同样连上（不停在「Resume 中…」）", async ({ page }) => {
  await page.goto("/");
  await open(page, "排查 Redis 连接池泄漏");
  await page.evaluate(() => {
    (window.__cvCcrpc as unknown as { resumeNamedInReady: boolean }).resumeNamedInReady = true;
  });
  await page.getByTestId("resume-button").click();
  await expect(page.getByTestId("header").getByTestId("connect-close")).toBeVisible();
  await expect(page.getByTestId("resume-pending")).toHaveCount(0);
  await expect(page.getByTestId("connect-input")).toBeEnabled();
});

test("Resume 失败：ccrpc 报 start_failed 时标题栏写明原因，连接丢掉，不显示「ccrpc 已退出」", async ({ page }) => {
  await page.goto("/");
  await open(page, JWT);
  await page.getByTestId("resume-button").click();
  const error = page.getByTestId("header").getByTestId("connect-error");
  await expect(error).toHaveText(
    "Resume 失败：No conversation found with session ID: c04e77a2-0003-4000-8000-000000000003",
  );
  // ccrpc closes the Pane and exits by itself: nothing is sent from here, the connection is dropped, Resume can be
  // tried again.
  expect(await lastSent(page, "get_screen")).toBeUndefined();
  expect(await lastSent(page, "exit")).toBeUndefined();
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  await expect(page.getByText("ccrpc 已退出")).toHaveCount(0);
  await expect(page.getByTestId("resume-button")).toBeEnabled();
});

test("Session 已结束的卡片上直接 Resume", async ({ page }) => {
  await page.goto("/");
  await open(page, "排查 Redis 连接池泄漏");
  await page.getByTestId("resume-button").click();
  await expect(page.getByTestId("connect-input")).toBeEnabled();
  // `/exit` ends claude (its Pane stays, out of `ccrpc ls`): the card, with Resume on it.
  await send(page, "/exit");
  const card = page.getByTestId("session-exit");
  await expect(card).toContainText("Session 已结束");
  await expect(page.getByTestId("proc-exit")).toHaveCount(0);
  await card.getByTestId("session-exit-resume").click();
  await expect(page.getByTestId("session-exit")).toHaveCount(0);
  await expect(page.getByTestId("connect-input")).toBeEnabled();
  await expect(page.getByTestId("connect-close")).toBeVisible();
  expect((await probe(page)).guiLog.filter((l) => l === "resume 91d5be38-0004-4000-8000-000000000004")).toHaveLength(2);
});

test("切回窗口时当前 Session 无事而别处在等作答：顶部一行提示，点击切过去", async ({ page }) => {
  await page.goto("/");
  // The LCP Live Session waits: no hint while it is the one on screen.
  await open(page, NO_PLUGIN);
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await page.waitForTimeout(200);
  await expect(page.getByTestId("elsewhere")).toHaveCount(0);

  await open(page, NOT_LISTED);
  // With the list hidden the toolbar floats over the conversation's top strip: the line stays clear of it, under the header.
  await page.keyboard.press("Control+Meta+L");
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await expect(page.getByTestId("elsewhere")).toHaveText(`${NO_PLUGIN} 在等你作答，点此切过去`);
  const line = (await page.getByTestId("elsewhere").boundingBox())!;
  const tools = (await page.getByTestId("toolbar").boundingBox())!;
  const header = (await page.getByTestId("header").boundingBox())!;
  expect(line.y).toBeGreaterThanOrEqual(tools.y + tools.height);
  expect(line.y).toBeGreaterThanOrEqual(header.y + header.height - 1);
  await page.keyboard.press("Control+Meta+L");
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

test("iPhone 发起的连接：桌面显示状态、能打字", async ({ page }) => {
  await page.goto("/");
  await open(page, TOOLS);
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);

  await remoteConn(page, "tools");
  await expect(page.getByTestId("connect-panel")).toBeVisible();
  await expect(state(page)).toContainText("空闲");
  await inMenu(page, "connect-disconnect", (b) => expect(b).toBeVisible());

  await send(page, "MOCK-TOOL please");
  await expect(page.getByTestId("permission-card")).toBeVisible();
  expect(await lastSent(page, "prompt")).toMatchObject({ text: "MOCK-TOOL please" });
});

test("iPhone Disconnect：桌面不留「ccrpc 已退出」卡片，可再次 Connect", async ({ page }) => {
  await page.goto("/");
  await open(page, TOOLS);
  const conn = await remoteConn(page, "tools");
  await expect(state(page)).toContainText("空闲");

  await page.evaluate((c) => window.__cvCcrpc!.remoteDisconnect(c), conn);
  await expect(page.getByTestId("connect-panel")).toHaveCount(0);
  await expect(page.getByText("ccrpc 已退出")).toHaveCount(0);
  await inMenu(page, "connect-button", (b) => expect(b).toBeEnabled());
});

test("打开窗口时认领 Host 上的连接：状态和等待作答的卡片都在", async ({ page }) => {
  await page.addInitScript(
    (snapshot) => (window.__cvCcrpcPreset = { snapshot }),
    [
      {
        conn: "c3",
        holderId: "m8ded9f28b056",
        sessionId: "a7c63e92-0008-4000-8000-000000000008",
        cwd: "/work/homelab",
        claudeVersion: "2.1.0",
        ready: true,
        state: "waiting",
        waiting: null,
        mode: "manual",
        model: null,
        effort: null,
        contextTokens: null,
        requests: [
          {
            type: "permission_request",
            request_id: "toolu_snap",
            tool_name: "Bash",
            input: { command: "ls" },
            agent_id: null,
            options: [
              { id: "1", label: "Yes", meaning: "allow" },
              { id: "2", label: "No", meaning: "deny" },
            ],
          },
        ],
        progress: null,
        queue: [],
        codeCalls: [],
        tasks: [],
        lastTurn: null,
        procExit: null,
        startFailed: null,
        fatal: null,
        sessionExited: false,
        notices: [],
        feedbackDraft: null,
        fork: null,
        disconnected: false,
      },
    ],
  );
  await page.goto("/");
  await open(page, TOOLS);
  await expect(page.getByTestId("connect-panel")).toBeVisible();
  await expect(page.getByTestId("permission-card")).toBeVisible();
});
