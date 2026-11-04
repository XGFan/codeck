import { expect, test, type Locator, type Page } from "@playwright/test";
import type { JumpTarget } from "../src/ipc/bindings";

const RICH_TITLE = "为 /v1/chat 接口增加令牌桶限流中间件";
const FORK_TITLE = "认证模块重构（改用 JWT 轮换）";

async function openSession(page: Page, title: string) {
  await page.goto("/");
  const row = page.getByTestId("session-row").filter({ hasText: title });
  await row.click();
  await expect(page.getByTestId("header").getByRole("heading", { name: title })).toBeVisible();
  return (await row.getAttribute("data-session-id"))!;
}

/** Scrolls `scroller` by `dy` and resolves after the scroll event (and the virtualizer's re-render); false at the end. */
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

/** Scrolls a virtualized list upward from the bottom until `target` is rendered. */
async function reveal(scroller: Locator, target: Locator) {
  await expect(scroller.locator("[data-row]").first()).toBeVisible();
  await scrollBy(scroller, 1e7);
  for (let i = 0; i < 40 && (await target.count()) === 0; i++) {
    if (!(await scrollBy(scroller, -300))) break;
  }
  await target.first().scrollIntoViewIfNeeded();
}

async function jump(page: Page, target: Omit<JumpTarget, "inSubagent" | "inAbandonedBranch" | "needsHidden"> & Partial<JumpTarget>) {
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
  await expect(transcript.getByText("这会改动 handler 的签名，影响面较大。")).toBeVisible();
  await expect(transcript.getByText("先只加中间件，不要动 handler。")).toHaveCount(0);

  await switcher.getByRole("button", { name: "上一个分支" }).click();
  await expect(switcher.getByTestId("branch-index")).toHaveText("1 / 2");
  await expect(transcript.getByText("先只加中间件，不要动 handler。")).toBeVisible();
});

test("工具调用合并成组：组头一行写做了什么、对象和失败数，默认收起，点开看每次调用", async ({ page }) => {
  await openSession(page, RICH_TITLE);
  const transcript = page.getByTestId("transcript");
  await transcript.evaluate((e) => (e.scrollTop = 0));
  const groups = page.getByTestId("tool-group");
  const first = groups.filter({ hasText: "读取 1 个文件、运行 1 条命令" });
  await expect(first).toContainText("mod.rs");
  await expect(first).not.toHaveAttribute("data-open");
  const failed = groups.filter({ hasText: "1 失败" });
  await expect(failed).not.toHaveAttribute("data-open");
  await expect(failed.getByTestId("tool-call")).toHaveCount(0);
  await first.getByRole("button", { name: /读取 1 个文件/ }).click();
  await expect(first.getByTestId("tool-call")).toHaveCount(2);
  // Each call is one quiet line: verb and target, no 「完成」 badge.
  await expect(first.getByTestId("tool-call").first()).toContainText("读取");
  await expect(first.getByTestId("tool-call").first()).not.toContainText("完成");
});

test("打开 Subagent 面板，嵌套卡片推入面包屑，Esc 关闭", async ({ page }) => {
  await openSession(page, RICH_TITLE);
  const card = page.locator('[data-testid="subagent-card"][data-agent-id="ag-sync1"]');
  await reveal(page.getByTestId("transcript"), card);
  await expect(page.getByTestId("parallel-agents")).toContainText("并行 · 2 个 Subagent");
  await card.getByTestId("open-subagent").click();

  const panel = page.getByTestId("subagent-panel");
  await expect(panel).toBeVisible();
  const crumbs = panel.getByTestId("panel-breadcrumb");
  await expect(crumbs).toContainText("主对话");
  await expect(crumbs.getByTestId("crumb")).toHaveText(["Explore · 梳理现有中间件注册方式"]);
  await expect(card).toContainText("正在右侧查看");
  await expect(panel.getByTestId("task-prompt")).toContainText("找出 lumen-api 里所有 tower Layer 的注册位置与顺序。");
  await expect(panel.getByTestId("final-result")).toContainText("共 3 处注册");

  await panel.locator('[data-agent-id="ag-nested1"]').getByTestId("open-subagent").click();
  await expect(crumbs.getByTestId("crumb")).toHaveText(["Explore · 梳理现有中间件注册方式", "general-purpose · 核对 cors 层的配置来源"]);
  await expect(panel.getByTestId("panel-meta")).toContainText("嵌套深度 2");
  await expect(panel.getByTestId("panel-transcript").getByText("来自 config.toml 的 [cors] allowed_origins。").first()).toBeVisible();

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
  await card.getByTestId("card-to-notification").click();
  await expect(page.locator('[data-anchor="n-notify"][data-highlighted]')).toBeVisible();
  await page.locator('[data-anchor="n-notify"]').getByTestId("notification-to-card").click();
  await expect(page.locator('[data-anchor="n12|tu_agent_tests"][data-highlighted]')).toBeVisible();
});

test("后台命令的结束通知写「后台命令」和命令本身，不当成 agent", async ({ page }) => {
  await openSession(page, RICH_TITLE);
  const row = page.locator('[data-anchor="n-notify-bash"]');
  await reveal(page.getByTestId("transcript"), row);
  await expect(row.getByTestId("task-notification")).toHaveText(/^后台命令 cargo build --release > build\.log 2>&1 已完成/);
  await expect(row).not.toContainText("agent");
  await expect(page.locator('[data-anchor="n-notify"]').getByTestId("task-notification")).toContainText("后台 agent");
});

test("后台命令：标「后台」、展开写完整命令与任务，和结束通知互相跳转", async ({ page }) => {
  await openSession(page, RICH_TITLE);
  const call = page.locator('[data-testid="tool-call"][data-tool="Bash"]').filter({ hasText: "cargo build --release" });
  const group = page.getByTestId("tool-group").filter({ hasText: "cargo build --release > build.log" });
  await reveal(page.getByTestId("transcript"), group);
  await group.getByRole("button").first().click();
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

test("工具输入是网址时（如 Artifact）可点击打开，点击不展开这次调用", async ({ page }) => {
  await openSession(page, RICH_TITLE);
  const group = page.getByTestId("tool-group").filter({ hasText: "cargo build --release > build.log" });
  await reveal(page.getByTestId("transcript"), group);
  await group.getByRole("button").first().click();
  const call = page.locator('[data-testid="tool-call"][data-tool="Artifact"]');
  const link = call.getByTestId("tool-link");
  await expect(link).toHaveAttribute("href", "https://claude.ai/artifact/M0ckDesignCanvas");
  await expect(link).toHaveAttribute("target", "_blank");
  const popup = page.waitForEvent("popup");
  await link.click();
  await (await popup).close();
  await expect(call.getByTestId("tool-body")).toHaveCount(0);
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

test("pendingJump 展开折叠的工具组并切换到被放弃的 Branch", async ({ page }) => {
  const sid = await openSession(page, RICH_TITLE);
  await jump(page, { sessionId: sid, scope: { kind: "main" }, branchChoices: [], nodeId: "n08", toolUseId: "tu_edit2", agentPath: [] });
  const hit = page.locator('[data-anchor="n08|tu_edit2"][data-highlighted]');
  await expect(hit).toBeVisible();
  await expect(hit).toBeInViewport();
  await expect(page.getByTestId("tool-group").filter({ has: page.locator('[data-anchor="n08|tu_edit2"]') })).toHaveAttribute("data-open", "true");

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
  await jump(page, { sessionId: sid, scope: { kind: "main" }, branchChoices: [], nodeId: "n04", toolUseId: "tu_bash1", agentPath: [] });
  await expect(page.getByTestId("header").getByRole("heading", { name: RICH_TITLE })).toBeVisible();
  const hit = page.locator('[data-anchor="n04|tu_bash1"][data-highlighted]');
  await expect(hit).toBeVisible();
  await expect(hit).toBeInViewport();
});

test("Teammate 消息显示为可折叠卡片而非用户提问", async ({ page }) => {
  await openSession(page, RICH_TITLE);
  const card = page.getByTestId("teammate-message");
  await reveal(page.getByTestId("transcript"), card);
  await expect(card).toContainText("Teammate · docs-writer");
  await expect(card.getByTestId("teammate-preview")).toHaveText("限流文档已更新");
  await expect(card).not.toContainText("你 ·");
  await expect(card.getByTestId("teammate-body")).toHaveCount(0);
  await card.getByRole("button", { expanded: false }).click();
  const body = card.getByTestId("teammate-body");
  await expect(body).toContainText("已在 docs/rate-limit.md 补充配置说明");
  await expect(body.locator("li")).toHaveCount(2);
  await expect(body).not.toContainText("This came from another Claude session");
});

test("当前 Session 被移除后对话区回到空状态", async ({ page }) => {
  const sid = await openSession(page, RICH_TITLE);
  await expect(page.getByTestId("transcript").locator("[data-row]").first()).toBeVisible();
  await page.evaluate((id) => window.__cvMock!.emit("sessionsChanged", { changed: [], removed: [id], projectsChanged: false }), sid);
  await expect(page.getByTestId("transcript-empty")).toBeVisible();
  expect(await page.evaluate(() => window.__cvStore!.getState().sessionId)).toBeNull();
});

test("对话不折叠：过程与回答按顺序排开；回答下方是复制与时间；你的消息时间在气泡下方；回复没有「Claude · 时间」抬头", async ({ page }) => {
  await openSession(page, RICH_TITLE);
  const t = page.getByTestId("transcript");
  await scrollBy(t, 1e7);
  await expect(t.getByTestId("worked")).toHaveCount(0);
  const last = t.locator('[data-row="tool"], [data-row="group"]').filter({ hasText: "config.rs" });
  await expect(last).toContainText("读取 1 个文件、编辑 1 个文件");
  await expect(t.getByText("默认值保持 120，未配置时沿用。")).toBeVisible();
  const actions = t.getByTestId("answer-actions").last();
  await expect(actions).toContainText("复制");
  await expect(actions).toContainText(/\d\d:\d\d/);
  const prompt = t.locator('[data-row="prompt"]').filter({ hasText: "接下来把限流参数" });
  const bubble = (await prompt.getByTestId("user-bubble").boundingBox())!;
  expect((await prompt.getByTestId("prompt-meta").boundingBox())!.y).toBeGreaterThan(bubble.y + bubble.height - 1);
  await expect(t.getByText(/^Claude · \d/)).toHaveCount(0);
});

test("提问卡片：题目很长折行时，第二行从标签下方的最左边开始", async ({ page }) => {
  await page.setViewportSize({ width: 1100, height: 900 });
  const sid = await openSession(page, RICH_TITLE);
  await page.evaluate((sid) => {
    const question = "模型与思考等级的选择器，按画布上这版（参考 Claude Code Desktop 文档）实现，还是你先发一张 Desktop 的截图我照着改？这一句要足够长才能折行。";
    const input = { questions: [{ question, header: "选择器", multiSelect: false, options: [{ label: "按这版实现", description: "一个选择器" }, { label: "我发截图再调", description: "先改画布" }] }] };
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
        blocks: [{ kind: "toolCall", toolUseId: "toolu_long_ask", name: "AskUserQuestion", inputJson: JSON.stringify(input), inputTruncated: false, result: null, subagentId: null, workflowRunId: null, notificationNodeId: null }],
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
