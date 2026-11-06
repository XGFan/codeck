import { expect, test, type Locator, type Page } from "@playwright/test";
import { openSession, RICH_TITLE, reveal, scrollBy } from "./helpers";
import type { JumpTarget } from "../src/ipc/bindings";

const FORK_TITLE = "认证模块重构（改用 JWT 轮换）";

async function jump(
  page: Page,
  target: Omit<JumpTarget, "inSubagent" | "inAbandonedBranch" | "needsHidden"> & Partial<JumpTarget>,
) {
  await page.evaluate(
    (t) =>
      window.__cvStore!.getState().jumpTo({
        inSubagent: t.agentPath.length > 0,
        inAbandonedBranch: false,
        needsHidden: false,
        ...t,
      }),
    target,
  );
}

test("切换 Branch 后后续内容随之改变", async ({ page }) => {
  await openSession(page, RICH_TITLE);
  const transcript = page.getByTestId("transcript");
  await transcript.evaluate((e) => (e.scrollTop = 0));
  const switcher = page.getByTestId("branch-switcher");
  await expect(switcher.getByTestId("branch-index")).toHaveText("1 / 2");
  await expect(transcript.getByText("先只加中间件，不要动 handler。")).toBeVisible();

  await switcher.getByRole("button", { name: "下一个分支" }).click();
  await expect(switcher.getByTestId("branch-index")).toHaveText("2 / 2");
  await expect(transcript.getByText("顺便把 handler 里重复的鉴权逻辑也一起收敛掉。")).toBeVisible();
  // That branch's turn is a fold of its own, closed as it opens.
  await transcript.locator('[data-testid="turn-fold"][aria-expanded="false"]').first().click();
  await expect(transcript.getByText("这会改动 handler 的签名，影响面较大。")).toBeVisible();
  await expect(transcript.getByText("先只加中间件，不要动 handler。")).toHaveCount(0);

  await switcher.getByRole("button", { name: "上一个分支" }).click();
  await expect(switcher.getByTestId("branch-index")).toHaveText("1 / 2");
  await expect(transcript.getByText("先只加中间件，不要动 handler。")).toBeVisible();
});

test("展开的过程里每次工具调用各占一行，不再合并成组：一行写动作和对象，不带「完成」", async ({ page }) => {
  await openSession(page, RICH_TITLE);
  const t = page.getByTestId("transcript");
  await t.evaluate((e) => (e.scrollTop = 0));
  await expect(t.locator('[data-row="tool"]').first()).toBeVisible();
  await expect(t.getByTestId("tool-group")).toHaveCount(0);
  const read = t.locator('[data-row="tool"]').filter({ hasText: "mod.rs" }).first();
  await expect(read).toContainText("读取");
  await expect(read).not.toContainText("完成");
  // A failed call stays collapsed, marked on its line.
  const failed = t.locator('[data-row="tool"]').filter({ hasText: "cargo build" }).filter({ hasText: "退出码" });
  await expect(failed).toHaveCount(1);
  await expect(failed.getByTestId("tool-body")).toHaveCount(0);
});

test("打开 Subagent 面板，嵌套卡片推入面包屑，Esc 关闭", async ({ page }) => {
  await openSession(page, RICH_TITLE);
  const card = page.locator('[data-testid="subagent-card"][data-agent-id="ag-sync1"]');
  await reveal(page.getByTestId("transcript"), card);
  await expect(page.getByTestId("parallel-agents")).toContainText("并行 · 2 个 Subagent");
  // Just the label and the cards: no line along their left.
  await expect(page.getByTestId("parallel-agents")).toHaveCSS("border-left-width", "0px");
  await card.getByTestId("open-subagent").click();

  const panel = page.getByTestId("subagent-panel");
  await expect(panel).toBeVisible();
  // The panel leaves the Session list and the toolbar as they were.
  await expect(page.getByTestId("pane-sessions")).toBeVisible();
  await expect(page.getByRole("button", { name: "隐藏 Session 列表" })).toBeEnabled();
  const crumbs = panel.getByTestId("panel-breadcrumb");
  await expect(crumbs).toContainText("主对话");
  await expect(crumbs.getByTestId("crumb")).toHaveText(["Explore · 梳理现有中间件注册方式"]);
  await expect(card).toContainText("正在右侧查看");
  await expect(panel.getByTestId("task-prompt")).toContainText("找出 lumen-api 里所有 tower Layer 的注册位置与顺序。");
  await expect(panel.getByTestId("final-result")).toContainText("共 3 处注册");
  await expect(card.getByTestId("run-status")).toHaveText("已完成");
  await expect(panel.getByTestId("panel-meta").getByTestId("run-status")).toHaveText("已完成");

  await panel.locator('[data-agent-id="ag-nested1"]').getByTestId("open-subagent").click();
  await expect(crumbs.getByTestId("crumb")).toHaveText([
    "Explore · 梳理现有中间件注册方式",
    "general-purpose · 核对 cors 层的配置来源",
  ]);
  await expect(panel.getByTestId("panel-meta")).toContainText("嵌套深度 2");
  await expect(
    panel.getByTestId("panel-transcript").getByText("来自 config.toml 的 [cors] allowed_origins。").first(),
  ).toBeVisible();

  await crumbs.getByTestId("crumb").first().click();
  await expect(crumbs.getByTestId("crumb")).toHaveCount(1);

  await page.keyboard.press("Escape");
  await expect(panel).toHaveCount(0);
});

test("Workflow 卡片中的 agent 在面板中打开", async ({ page }) => {
  await openSession(page, RICH_TITLE);
  const wf = page.getByTestId("workflow-card");
  await reveal(page.getByTestId("transcript"), wf);
  await expect(wf).toContainText("3 阶段 · 2 个 agent");
  await wf.getByTestId("workflow-agent").filter({ hasText: "安全评审" }).click();
  await expect(page.getByTestId("panel-breadcrumb").getByTestId("crumb")).toHaveText(["security-reviewer"]);
});

test("后台 agent 卡片与结果通知互相跳转", async ({ page }) => {
  await openSession(page, RICH_TITLE);
  const card = page.locator('[data-testid="subagent-card"][data-agent-id="ag-async1"]');
  await reveal(page.getByTestId("transcript"), card);
  // Ended with its notification: the result is the notification's, not the launch message.
  await expect(card.getByTestId("run-status")).toHaveText("已完成");
  await expect(card).toContainText("测试代理完成");
  await card.getByTestId("card-to-notification").click();
  await expect(page.locator('[data-anchor="n-notify"][data-highlighted]')).toBeVisible();
  await page.locator('[data-anchor="n-notify"]').getByTestId("notification-to-card").click();
  await expect(page.locator('[data-anchor="n12|tu_agent_tests"][data-highlighted]')).toBeVisible();
});

test("SendMessage 续接后的结束通知：派出位置仍是 Agent 调用，另有续接位置；面板里显示主对话发来的消息", async ({
  page,
}) => {
  const sid = await openSession(page, RICH_TITLE);
  await jump(page, {
    sessionId: sid,
    scope: { kind: "main" },
    branchChoices: [],
    nodeId: "n-notify2",
    toolUseId: null,
    agentPath: [],
  });
  const note = page.locator('[data-anchor="n-notify2"]');
  await expect(note).toBeVisible();
  await note.getByTestId("notification-to-resume").click();
  await expect(page.locator('[data-anchor="n14|tu_sendmsg"][data-highlighted]')).toBeVisible();
  await jump(page, {
    sessionId: sid,
    scope: { kind: "main" },
    branchChoices: [],
    nodeId: "n-notify2",
    toolUseId: null,
    agentPath: [],
  });
  await note.getByTestId("notification-to-card").click();
  await expect(page.locator('[data-anchor="n12|tu_agent_tests"][data-highlighted]')).toBeVisible();

  await page.locator('[data-testid="subagent-card"][data-agent-id="ag-async1"]').getByTestId("open-subagent").click();
  const panel = page.getByTestId("subagent-panel");
  const prompts = panel.getByTestId("task-prompt");
  await expect(prompts).toHaveCount(2);
  await expect(prompts.nth(1)).toContainText("主对话发来的消息");
  await expect(prompts.nth(1)).toContainText("再补一个窗口恢复的边界用例。");
});

test("后台命令的结束通知写「后台命令」和命令本身，不当成 agent", async ({ page }) => {
  await openSession(page, RICH_TITLE);
  const row = page.locator('[data-anchor="n-notify-bash"]');
  await reveal(page.getByTestId("transcript"), row);
  await expect(row.getByTestId("task-notification")).toHaveText(
    /^后台命令 cargo build --release > build\.log 2>&1 已完成/,
  );
  await expect(row).not.toContainText("agent");
  await expect(page.locator('[data-anchor="n-notify"]').getByTestId("task-notification")).toContainText("后台 agent");
});

test("后台命令：标「后台」、展开写完整命令与任务，和结束通知互相跳转", async ({ page }) => {
  await openSession(page, RICH_TITLE);
  const call = page.locator('[data-testid="tool-call"][data-tool="Bash"]').filter({ hasText: "cargo build --release" });
  await reveal(page.getByTestId("transcript"), call);
  await expect(call.getByTestId("bash-background")).toHaveText("后台");
  await call.getByRole("button").first().click();
  await expect(call.getByTestId("tool-body")).toContainText("$ cargo build --release > build.log 2>&1");
  await expect(call.getByTestId("bash-background-note")).toContainText("在后台运行（任务 b-build1）");
  await expect(call.getByTestId("terminal")).toHaveCount(0);
  await call.getByTestId("bash-to-notification").click();
  await expect(page.locator('[data-anchor="n-notify-bash"][data-highlighted]')).toBeVisible();
  await page.locator('[data-anchor="n-notify-bash"]').getByTestId("notification-to-card").click();
  await expect(page.locator('[data-anchor="n12|tu_bg_build"][data-highlighted]')).toBeVisible();
});

test("思考：一行写「思考 · N 行」，首尾空行不计也不显示；正文与输入框同为 14px", async ({ page }) => {
  await openSession(page, RICH_TITLE);
  const t = page.getByTestId("transcript");
  const lines = t.getByTestId("thinking-lines").first();
  await reveal(t, lines);
  await expect(lines).toHaveText("2 行");
  await t.getByRole("button", { name: /思考/ }).first().click();
  await expect(t.getByTestId("thinking-text").first()).toHaveText(
    "需要先看现有中间件的注册方式，\n再决定是 tower Layer 还是 axum from_fn。",
  );
  const size = (text: string) =>
    t.getByText(text, { exact: true }).evaluate((el) => getComputedStyle(el.closest(".min-w-0")!).fontSize);
  expect(await size("先看一下现有中间件的组织方式。")).toBe("14px");
});

test("你的消息里 ` 与 ``` 包着的内容显示为代码，反引号不显示，其余照原样", async ({ page }) => {
  await openSession(page, RICH_TITLE);
  const t = page.getByTestId("transcript");
  const bubble = t.getByTestId("user-bubble").filter({ hasText: "接下来把限流参数" });
  await reveal(t, bubble);
  await expect(bubble.getByTestId("prompt-code")).toHaveText("[rate_limit] per_minute");
  await expect(bubble.getByTestId("prompt-code-block")).toHaveText("[rate_limit]\nper_minute = 120");
  await expect(bubble).not.toContainText("`");
  await expect(bubble).toContainText("没配置时沿用 120。");
  expect(await bubble.getByTestId("prompt-code-block").evaluate((el) => getComputedStyle(el).fontFamily)).toMatch(
    /mono/i,
  );
});

test("折叠行的箭头大小不随摘要长短变化", async ({ page }) => {
  await page.setViewportSize({ width: 600, height: 1400 });
  await openSession(page, RICH_TITLE, { folded: true });
  const folds = page.getByTestId("turn-fold");
  await expect(folds.first()).toBeVisible();
  const widths = await folds.evaluateAll((els) =>
    els.map((el) => el.querySelector("svg")!.getBoundingClientRect().width),
  );
  expect(widths.length).toBeGreaterThan(1);
  for (const w of widths) expect(w).toBe(13);
});

test("工具输入里有网址时（如 Artifact 的 url）可点击打开，点击不展开这次调用；摘要是别的字段时网址跟在后面", async ({
  page,
}) => {
  await openSession(page, RICH_TITLE);
  const calls = page.locator('[data-testid="tool-call"][data-tool="Artifact"]');
  await reveal(page.getByTestId("transcript"), calls);
  const [byUrl, byPath] = [
    calls.filter({ hasNotText: "canvas.json" }),
    calls.filter({ hasText: "project/canvas.json" }).filter({ hasNotText: "/private/" }),
  ];
  for (const call of [byUrl, byPath]) {
    await expect(call.getByTestId("tool-link")).toHaveAttribute("href", "https://claude.ai/artifact/M0ckDesignCanvas");
    await expect(call.getByTestId("tool-link")).toHaveAttribute("target", "_blank");
  }
  const popup = page.waitForEvent("popup");
  await byPath.getByTestId("tool-link").click();
  await (await popup).close();
  await expect(byPath.getByTestId("tool-body")).toHaveCount(0);
  // A long summary (an absolute path) shrinks with the link and stays inside its row, clear of the time.
  const long = calls.filter({ hasText: "/private/tmp/" });
  const row = (await long.getByRole("button").first().boundingBox())!;
  for (const el of [long.locator("code").first(), long.getByTestId("tool-link")]) {
    const b = (await el.boundingBox())!;
    expect(b.width).toBeGreaterThan(20);
    expect(b.x + b.width).toBeLessThanOrEqual(row.x + row.width);
  }
});

test("对话宽度：拖动列的边缘调节（两侧对称、居中），重开后保持；双击恢复 720px；键盘 ← / → 也可调", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await openSession(page, RICH_TITLE, { folded: true });
  const col = page.getByTestId("conversation-column");
  const width = async () => Math.round((await col.boundingBox())!.width);
  expect(await width()).toBe(720);
  const center = async () => {
    const b = (await col.boundingBox())!;
    return Math.round(b.x + b.width / 2);
  };
  const c0 = await center();
  const h = (await page.getByTestId("width-handle-right").boundingBox())!;
  await page.mouse.move(h.x + h.width / 2, h.y + 200);
  await page.mouse.down();
  await page.mouse.move(h.x + h.width / 2 + 100, h.y + 200, { steps: 5 });
  await page.mouse.up();
  expect(await width()).toBe(920);
  expect(await center()).toBe(c0);
  // The rows follow (the column's content is this wide).
  const row = page.getByTestId("transcript").locator("[data-row]").first();
  expect(Math.round((await row.boundingBox())!.width)).toBe(920);

  await openSession(page, RICH_TITLE, { folded: true });
  expect(await width()).toBe(920);

  await page.getByTestId("width-handle-left").dblclick();
  expect(await width()).toBe(720);
  await page.getByTestId("width-handle-left").focus();
  await page.keyboard.press("ArrowLeft");
  expect(await width()).toBe(760);
});

test("显示系统消息开关控制隐藏条目", async ({ page }) => {
  await openSession(page, RICH_TITLE);
  const transcript = page.getByTestId("transcript");
  await transcript.evaluate((e) => (e.scrollTop = 0));
  await expect(transcript.getByText("可用技能：review、simplify、security-review（共 3 项）")).toHaveCount(0);
  await page.getByTestId("header-more").click();
  await page.getByRole("menuitemcheckbox", { name: "显示系统消息" }).click();
  await transcript.evaluate((e) => (e.scrollTop = 0));
  await expect(transcript.getByText("可用技能：review、simplify、security-review（共 3 项）")).toBeVisible();
  await page.getByTestId("header-more").click();
  await page.getByRole("menuitemcheckbox", { name: "显示系统消息" }).click();
  await expect(transcript.getByText("可用技能：review、simplify、security-review（共 3 项）")).toHaveCount(0);
});

test("只有外壳的系统记录按类型写出文案，孤立的工具结果标「工具结果」并随系统消息显示", async ({ page }) => {
  await openSession(page, RICH_TITLE);
  const transcript = page.getByTestId("transcript");
  const killed = transcript.locator('[data-node-id="n-killed"]');
  const orphan = transcript.locator('[data-node-id="n-orphan-result"]');
  await reveal(transcript, killed, "top");
  await expect(killed).toContainText("已终止所有后台 agent");
  await expect(orphan).toHaveCount(0);
  await page.getByTestId("header-more").click();
  await page.getByRole("menuitemcheckbox", { name: "显示系统消息" }).click();
  await reveal(transcript, orphan, "top");
  await expect(orphan).toContainText("工具结果");
  await expect(orphan).toContainText("孤立的工具结果：调用不在当前显示的路径上");
});

test("Fork 继承的历史默认折叠，可展开与收起", async ({ page }) => {
  await openSession(page, FORK_TITLE);
  const transcript = page.getByTestId("transcript");
  const banner = page.getByTestId("fork-banner");
  await expect(banner).toContainText("Fork 自");
  await expect(banner).toContainText("重构认证模块：拆分 token 校验与会话存储");
  await expect(page.getByTestId("fork-divider")).toContainText("以下为本 Session 新增内容");
  const inherited = transcript.getByText("把 auth 模块里 token 校验和会话存储拆开，先给出拆分方案。");
  await expect(inherited).toHaveCount(0);

  await banner.getByTestId("toggle-inherited").click();
  await transcript.evaluate((e) => (e.scrollTop = 0));
  await expect(inherited).toBeVisible();
  await expect(banner.getByTestId("toggle-inherited")).toHaveText("收起继承的 6 条消息");

  await banner.getByTestId("toggle-inherited").click();
  await expect(inherited).toHaveCount(0);
});

test("pendingJump 进入嵌套 Subagent 时打开面板并高亮目标", async ({ page }) => {
  const sid = await openSession(page, RICH_TITLE);
  await jump(page, {
    sessionId: sid,
    scope: { kind: "subagent", agentId: "ag-nested1" },
    branchChoices: [],
    nodeId: "ag1n-a1",
    toolUseId: null,
    agentPath: ["ag-sync1", "ag-nested1"],
  });
  const panel = page.getByTestId("subagent-panel");
  await expect(panel.getByTestId("panel-breadcrumb").getByTestId("crumb")).toHaveCount(2);
  const target = panel.locator('[data-anchor="ag1n-a1"][data-highlighted]');
  await expect(target.first()).toBeVisible();
  // The flash ring clears after 2 s.
  await expect(panel.locator('[data-anchor="ag1n-a1"][data-highlighted]')).toHaveCount(0, { timeout: 4000 });
  expect(await page.evaluate(() => window.__cvStore!.getState().pendingJump)).toBeNull();
});

test("pendingJump 展开折叠的那一轮并切换到被放弃的 Branch", async ({ page }) => {
  const sid = await openSession(page, RICH_TITLE);
  await jump(page, {
    sessionId: sid,
    scope: { kind: "main" },
    branchChoices: [],
    nodeId: "n08",
    toolUseId: "tu_edit2",
    agentPath: [],
  });
  const hit = page.locator('[data-anchor="n08|tu_edit2"][data-highlighted]');
  await expect(hit).toBeVisible();
  await expect(hit).toBeInViewport();
  await expect(
    page.locator('[data-row="tool"]').filter({ has: page.locator('[data-anchor="n08|tu_edit2"]') }),
  ).toHaveCount(1);

  await jump(page, {
    sessionId: sid,
    scope: { kind: "main" },
    branchChoices: [{ anchorKey: "n04", headId: "n05x" }],
    nodeId: "n05y",
    toolUseId: "tu_alt_edit",
    agentPath: [],
    inAbandonedBranch: true,
  });
  await expect(page.locator('[data-anchor="n05y|tu_alt_edit"][data-highlighted]')).toBeVisible();
  await expect(page.getByTestId("branch-index")).toHaveText("2 / 2");
});

test("从其他 Session 发起 pendingJump 时打开目标 Session 并定位", async ({ page }) => {
  const sid = await openSession(page, RICH_TITLE);
  await page.getByTestId("session-row").filter({ hasText: FORK_TITLE }).click();
  await expect(page.getByTestId("fork-banner")).toBeVisible();
  await jump(page, {
    sessionId: sid,
    scope: { kind: "main" },
    branchChoices: [],
    nodeId: "n04",
    toolUseId: "tu_bash1",
    agentPath: [],
  });
  await expect(page.getByTestId("header").getByRole("heading", { name: RICH_TITLE })).toBeVisible();
  const hit = page.locator('[data-anchor="n04|tu_bash1"][data-highlighted]');
  await expect(hit).toBeVisible();
  await expect(hit).toBeInViewport();
});

test("Teammate 的报告是一张卡片：名字、「报告」、summary 作标题，正文直接显示，不当作你的消息", async ({ page }) => {
  await openSession(page, RICH_TITLE);
  const card = page.getByTestId("teammate-message").first();
  await reveal(page.getByTestId("transcript"), card);
  await expect(card).toContainText("docs-writer");
  await expect(card.getByTestId("message-tag")).toHaveText("报告");
  await expect(card.getByTestId("message-title")).toHaveText("限流文档已更新");
  await expect(card).not.toContainText("你 ·");
  const body = card.getByTestId("teammate-body");
  await expect(body).toContainText("已在 docs/rate-limit.md 补充配置说明");
  await expect(body.locator("li")).toHaveCount(2);
  // Prose size, as Claude's text.
  await expect(body.locator("p").first()).toHaveCSS("font-size", "14px");
  await expect(body).not.toContainText("This came from another Claude session");
});

test("Teammate：一轮的结果是一张卡片，空闲、失败、分配任务与退出各一行；它的卡片带标签、状态与最近的结果", async ({
  page,
}) => {
  await openSession(page, RICH_TITLE);
  const transcript = page.getByTestId("transcript");
  const done = page.getByTestId("teammate-done").filter({ hasText: "限流文档写好了" });
  await reveal(transcript, done);
  await expect(done).toContainText("docs-writer");
  await expect(done).toContainText("完成");
  await expect(done).not.toContainText("idle_notification");
  await expect(done.getByTestId("teammate-result").locator("li")).toHaveCount(5);
  await expect(done.getByTestId("message-tag")).toHaveText("完成");
  // A long result starts clamped.
  await done.getByRole("button", { name: "展开全文" }).click();
  await expect(done.getByRole("button", { name: "收起" })).toHaveAttribute("aria-expanded", "true");

  // A report and the end of the round in one message: one card, the report folded inside it.
  const pair = page.getByTestId("teammate-done").filter({ hasText: "Retry-After 的示例写好了" });
  await reveal(transcript, pair);
  await expect(page.getByTestId("teammate-message").filter({ hasText: "429 示例已补" })).toHaveCount(0);
  const report = pair.getByTestId("teammate-done-report");
  await expect(report).toContainText("报告");
  await expect(report).toContainText("429 示例已补");
  await expect(report.locator("table")).toHaveCount(0);
  await report.getByRole("button", { expanded: false }).click();
  await expect(report.locator("table")).toBeVisible();

  const event = (kind: string) => page.locator(`[data-testid="teammate-event"][data-kind="${kind}"]`);
  await reveal(transcript, event("exited"));
  await expect(event("idle")).toContainText("docs-writer 空闲");
  await expect(event("task")).toContainText("team-lead 分配任务 · #6 补充 Retry-After 的示例");
  await expect(event("failed")).toContainText("docs-writer 失败 · API Error: Connection dropped (ECONNRESET)");
  // Its shutdown_approved and the system's teammate_terminated: one line.
  await expect(event("exited")).toHaveCount(1);
  await expect(event("exited")).toContainText("docs-writer 已退出");

  const card = page.locator('[data-testid="subagent-card"][data-agent-id="ag-docs"]');
  await reveal(transcript, card);
  await expect(card.getByTestId("teammate-badge")).toHaveText("Teammate · docs-writer");
  await expect(card.getByTestId("run-status")).toHaveAttribute("data-state", "exited");
  await expect(card.getByTestId("run-status")).toHaveText("已退出");
  await expect(card).toContainText("限流文档写好了");
  await expect(card).not.toContainText("Spawned successfully");
});

test("其他 Agent 经 SendMessage 发来的消息是卡片，写明来源，不当作你的消息", async ({ page }) => {
  await openSession(page, RICH_TITLE);
  const transcript = page.getByTestId("transcript");
  const peer = page.getByTestId("peer-message");
  await reveal(transcript, peer.first());
  const session = peer.filter({ hasText: "codeck-99" });
  await expect(session.getByTestId("message-tag")).toHaveText("会话");
  await expect(session).not.toContainText("你 ·");
  const body = session.getByTestId("teammate-body");
  await expect(body).toContainText("限流文档我这边也改了一处，合并前先拉一下。");
  await expect(body.locator("li")).toHaveCount(1);
  await expect(body).not.toContainText("cross-session-message");
  await expect(body).not.toContainText("This came from another Claude session");
  const agent = peer.filter({ hasText: "ag-async1" });
  await expect(agent.getByTestId("message-tag")).toHaveText("Agent");
  await expect(agent.getByTestId("teammate-body")).toHaveText("集成测试还差窗口恢复的用例，正在补。");
});

test("SendMessage 写明发给谁（teammate 带颜色、会话写名字、Subagent 写续接）与摘要；展开是消息正文与是否送达", async ({
  page,
}) => {
  const sid = await openSession(page, RICH_TITLE);
  const at = async (toolUseId: string) => {
    await jump(page, {
      sessionId: sid,
      scope: { kind: "main" },
      branchChoices: [],
      nodeId: "n14",
      toolUseId,
      agentPath: [],
    });
    const c = page.locator(`[data-anchor="n14|${toolUseId}"] [data-testid="tool-call"]`);
    await expect(c).toBeVisible();
    return c;
  };
  // A jump to a call may already have opened it.
  const open = async (c: Locator) => {
    const toggle = c.locator("button[aria-expanded]").first();
    if ((await toggle.getAttribute("aria-expanded")) === "false") await toggle.click();
    await expect(toggle).toHaveAttribute("aria-expanded", "true");
  };

  const team = await at("tu_send_team");
  await expect(team).toContainText("发消息给");
  await expect(team).not.toContainText("SendMessage");
  await expect(team.getByTestId("send-target")).toHaveText("docs-writer");
  await expect(team.getByTestId("send-target").locator("span").first()).toHaveCSS("background-color", "rgb(0, 128, 0)");
  await expect(team).toContainText("补 Retry-After 示例");
  await open(team);
  const body = team.getByTestId("tool-body");
  await expect(body.locator("strong")).toHaveText("429 响应");
  await expect(body).not.toContainText('"to"');
  await expect(team.getByTestId("send-status")).toHaveText("已送达");

  const session = await at("tu_send_session");
  await expect(session).toContainText("发消息给会话");
  await expect(session.getByTestId("send-target")).toHaveText("codeck-99");

  const resume = await at("tu_sendmsg");
  await expect(resume).toContainText("续接 Subagent");
  await open(resume);
  await expect(resume.getByTestId("send-status")).toContainText("已续接");
  await resume.getByRole("button", { name: "跳到结束通知 ↓" }).click();
  await expect(page.locator('[data-anchor="n-notify2"][data-highlighted]')).toBeVisible();
});

test("API 重试：放弃了的并进「API 错误」那一行，恢复了的是灰色的一行，点开列出每次；不显示 JSON", async ({ page }) => {
  const sid = await openSession(page, RICH_TITLE);
  const go = (nodeId: string) =>
    jump(page, { sessionId: sid, scope: { kind: "main" }, branchChoices: [], nodeId, toolUseId: null, agentPath: [] });

  await go("n-retry1");
  const error = page.getByTestId("api-error");
  await expect(error).toBeVisible();
  await expect(error).toContainText("API 错误 · 重试 2 次后放弃");
  await expect(page.getByTestId("transcript")).not.toContainText("retryAttempt");

  await go("n-retry3");
  const retry = page.getByTestId("api-retry");
  await expect(retry).toHaveCount(1);
  await expect(retry).toHaveText(/API 请求失败：529 Overloaded，重试 1 次后恢复/);
  await expect(retry).toHaveAttribute("data-recovered", "true");
  await retry.click();
  await expect(page.getByTestId("api-retry-list")).toHaveText(/第 1\/10 次 · 520 毫秒后重试/);
});

test("当前 Session 被移除后会话区回到空状态", async ({ page }) => {
  const sid = await openSession(page, RICH_TITLE);
  await expect(page.getByTestId("transcript").locator("[data-row]").first()).toBeVisible();
  await page.evaluate(
    (id) => window.__cvMock!.emit("sessionsChanged", { changed: [], removed: [id], projectsChanged: false }),
    sid,
  );
  await expect(page.getByTestId("transcript-empty")).toBeVisible();
  expect(await page.evaluate(() => window.__cvStore!.getState().sessionId)).toBeNull();
});

test("按轮折叠：每轮的过程收成「已处理 … · N 段说明、N 次工具调用」，点开按顺序排开；回答在外面，下方是复制与时间；你的消息时间在气泡下方", async ({
  page,
}) => {
  await openSession(page, RICH_TITLE, { folded: true });
  const t = page.getByTestId("transcript");
  await scrollBy(t, 1e7);
  const prompt = t.locator('[data-row="prompt"]').filter({ hasText: "接下来把限流参数" });
  const fold = t.locator('[data-row="fold"]').last();
  await expect(fold).toHaveText(/^已处理 \d+ (秒|分 \d\d 秒)\s?· .*次工具调用$/);
  await expect(t.locator('[data-row="tool"]').filter({ hasText: "config.rs" })).toHaveCount(0);
  await expect(t.getByText("默认值保持 120，未配置时沿用。")).toBeVisible();
  const actions = t.getByTestId("answer-actions").last();
  await expect(actions).toContainText("复制");
  await expect(actions).toContainText(/\d\d:\d\d/);
  await fold.getByTestId("turn-fold").click();
  // Opened: each call on its own line, not grouped.
  await expect(t.locator('[data-row="tool"]').filter({ hasText: "config.rs" })).toHaveCount(2);
  const bubble = (await prompt.getByTestId("user-bubble").boundingBox())!;
  expect((await prompt.getByTestId("prompt-meta").boundingBox())!.y).toBeGreaterThan(bubble.y + bubble.height - 1);
  await expect(t.getByText(/^Claude · \d/)).toHaveCount(0);
});

test("搜索命中折叠里的内容：自动展开那一轮并定位", async ({ page }) => {
  await openSession(page, RICH_TITLE, { folded: true });
  await page.keyboard.press("Meta+f");
  await page.keyboard.type("DefaultClock");
  await expect(page.locator('[data-row="text"]').filter({ hasText: "DefaultClock 在 governor 0.8" })).toBeVisible();
});

test("提问卡片：题目很长折行时，第二行从标签下方的最左边开始", async ({ page }) => {
  await page.setViewportSize({ width: 1100, height: 900 });
  const sid = await openSession(page, RICH_TITLE);
  await page.evaluate((sid) => {
    const question =
      "模型与思考等级的选择器，按画布上这版（参考 Claude Code Desktop 文档）实现，还是你先发一张 Desktop 的截图我照着改？这一句要足够长才能折行。";
    const input = {
      questions: [
        {
          question,
          header: "选择器",
          multiSelect: false,
          options: [
            { label: "按这版实现", description: "一个选择器" },
            { label: "我发截图再调", description: "先改画布" },
          ],
        },
      ],
    };
    window.__cvMock!.appendNode(sid, {
      id: "e2e-long-ask",
      timestampMs: Date.now(),
      hidden: false,
      inherited: false,
      body: {
        kind: "assistant",
        messageId: "m-e2e-long-ask",
        model: null,
        usage: null,
        isApiError: false,
        blocks: [
          {
            kind: "toolCall",
            toolUseId: "toolu_long_ask",
            name: "AskUserQuestion",
            inputJson: JSON.stringify(input),
            inputTruncated: false,
            result: null,
            subagentId: null,
            workflowRunId: null,
            notificationNodeId: null,
          },
        ],
      },
    });
    window.__cvMock!.emit("sessionsChanged", { changed: [sid], removed: [], projectsChanged: false });
  }, sid);
  const legend = page.locator('[data-anchor="e2e-long-ask|toolu_long_ask"]').getByTestId("ask-legend");
  await scrollBy(page.getByTestId("transcript"), 1e7);
  await expect(legend).toBeVisible();
  const lines = await legend.evaluate((el) => {
    const text = el.querySelector("span.font-medium")!;
    const rects = [...text.getClientRects()];
    return { box: el.getBoundingClientRect().left, rects: rects.map((r) => r.left) };
  });
  expect(lines.rects.length).toBeGreaterThan(1);
  expect(Math.abs(lines.rects[1]! - lines.box)).toBeLessThan(2);
});
