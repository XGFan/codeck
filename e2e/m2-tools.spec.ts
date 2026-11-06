import { expect, test, type Locator, type Page } from "@playwright/test";
import { openRich, openSession, reveal as revealIn } from "./helpers";

/** Scrolls the transcript down from the top, opening collapsed tool groups, until `target` is rendered. */
const reveal = (page: Page, target: Locator) => revealIn(page.getByTestId("transcript"), target, "top");

test("失败的 Bash 默认收起，展开后渲染 ANSI 颜色", async ({ page }) => {
  await openRich(page);
  const failed = page.locator('[data-tool="Bash"][data-failed]').first();
  await reveal(page, failed);
  await expect(failed.getByTestId("terminal")).toHaveCount(0);
  await failed.getByRole("button").first().click();
  const term = failed.getByTestId("terminal");
  await expect(term).toContainText("unresolved import");
  await expect(failed).toContainText("退出码 101");
  const colored = term.locator("span[style*='color']").filter({ hasText: "error[E0432]" });
  await expect(colored).toBeVisible();
  const color = await colored.evaluate((e) => getComputedStyle(e).color);
  expect(color).not.toBe(await term.evaluate((e) => getComputedStyle(e).color));
});

test("Edit 展开后显示带 +/- 的 diff", async ({ page }) => {
  await openRich(page);
  const edit = page.locator('[data-tool="Edit"]').filter({ hasText: "mod.rs" }).first();
  await reveal(page, edit);
  await edit.getByRole("button").first().click();
  await expect(edit.locator('[data-diff="add"]')).toHaveCount(1);
  await expect(edit.locator('[data-diff="del"]')).toHaveCount(1);
  await expect(edit.locator('[data-diff="add"]')).toContainText("rate_limit::layer()");
  await expect(edit).toContainText("+1");
  await expect(edit).toContainText("−1");
});

test("持久化输出可点击加载完整内容", async ({ page }) => {
  await openRich(page);
  const bash = page.locator('[data-tool="Bash"]').filter({ hasText: "cargo test --workspace" });
  await reveal(page, bash);
  await bash.getByRole("button").first().click();
  await expect(bash).toContainText("来源：tool-results/bd2x9k1.txt");
  await bash.getByRole("button", { name: /加载完整输出（共 180\.0 KB）/ }).click();
  await expect(bash).toContainText("412 passed");
  await expect(bash).toContainText("中间");
  await expect(bash.getByRole("button", { name: /加载完整输出/ })).toHaveCount(0);
});

test("复制 Bash 命令", async ({ page }) => {
  await openRich(page);
  const bash = page.locator('[data-tool="Bash"]').filter({ hasText: "cargo test --workspace" });
  await reveal(page, bash);
  await bash.getByRole("button").first().click();
  await bash.hover();
  await bash.getByRole("button", { name: "复制命令" }).click();
  const calls = await page.evaluate(() => window.__cvCalls ?? []);
  expect(calls.at(-1)).toEqual({ method: "copyText", args: ["cargo test --workspace"] });
});

test("你的消息里的 [Image #n] 显示成图片块：悬停预览、点击放大，下方不再另列缩略图", async ({ page }) => {
  await openSession(page, "Longhorn 卷扩容与备份策略");
  const bubble = page.getByTestId("user-bubble").filter({ hasText: "PVC" });
  const chip = bubble.getByTestId("prompt-image");
  await expect(chip).toHaveText("图片 #1");
  await expect(bubble).toContainText("（现在的用量见 图片 #1），并设置每日备份保留 7 份。");
  await expect(page.getByTestId("image-thumb")).toHaveCount(0);
  await chip.hover();
  await expect(page.getByTestId("prompt-image-preview").locator("img")).toBeVisible();
  await chip.click();
  await expect(page.getByTestId("image-large")).toBeVisible();
});

test("对话记录里的图片块离顶端太近时，预览出现在下方，不被裁掉也不被下一行盖住", async ({ page }) => {
  await openSession(page, "Longhorn 卷扩容与备份策略");
  const transcript = page.getByTestId("transcript");
  const chip = page.getByTestId("user-bubble").filter({ hasText: "PVC" }).getByTestId("prompt-image");
  const preview = page.getByTestId("prompt-image-preview");
  // Room above: the preview above the block.
  const top = await transcript.evaluate((e) => e.getBoundingClientRect().top);
  await chip.evaluate((e, top) => {
    const s = e.closest("[data-testid=transcript]")!;
    s.scrollTop += e.getBoundingClientRect().top - top - 300;
  }, top);
  await chip.hover();
  await expect(preview.locator("img")).toBeVisible();
  await expect(preview).not.toHaveAttribute("data-below");
  await page.mouse.move(0, 0);
  // Right under the top edge (a short window, so the transcript scrolls that far): below it, and drawn over what
  // follows.
  await page.setViewportSize({ width: 1280, height: 420 });
  await chip.evaluate((e, top) => {
    const s = e.closest("[data-testid=transcript]")!;
    s.scrollTop += e.getBoundingClientRect().top - top - 20;
  }, top);
  expect((await chip.evaluate((e) => e.getBoundingClientRect().top)) - top).toBeLessThan(80);
  await chip.hover();
  await expect(preview).toHaveAttribute("data-below", "true");
  await expect(preview.locator("img")).toBeVisible();
  const box = (await preview.boundingBox())!;
  expect(box.y).toBeGreaterThan(top);
  // Hit-tested with the preview taking the pointer for a moment (it lets the pointer through).
  const onTop = await preview.evaluate(
    (p, { x, y }) => {
      p.style.pointerEvents = "auto";
      const hit = !!document.elementFromPoint(x, y)?.closest("[data-testid=prompt-image-preview]");
      p.style.pointerEvents = "";
      return hit;
    },
    { x: box.x + box.width / 2, y: box.y + box.height - 8 },
  );
  expect(onTop).toBe(true);
});

test("图片缩略图渲染并可放大", async ({ page }) => {
  await openRich(page);
  const read = page.locator('[data-tool="Read"]').filter({ hasText: "grafana-ratelimit.png" });
  await reveal(page, read);
  await read.getByRole("button").first().click();
  const thumb = read.getByTestId("image-thumb").locator("img");
  await expect(thumb).toBeVisible();
  await read.getByTestId("image-thumb").click();
  await expect(page.getByTestId("image-large")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("image-large")).toHaveCount(0);
});

test("AskUserQuestion 单独显示问题并标出你的回答，任务清单显示状态", async ({ page }) => {
  await openRich(page);
  const ask = page.getByTestId("ask").filter({ hasText: "限流超限时应返回什么？" });
  await reveal(page, ask);
  // Shown expanded, outside any tool group.
  await expect(page.locator('[data-testid="tool-group"] [data-testid="ask"]')).toHaveCount(0);
  await expect(ask.getByTestId("ask-question")).toHaveCount(3);
  await expect(ask.locator("[data-chosen]").first()).toContainText("429 + Retry-After");
  await expect(ask.locator("[data-chosen]").nth(1)).toContainText("其他：先放内存，留好换 Redis 的接口");
  await expect(ask.getByTestId("ask-status")).toContainText(/你 · \d\d:\d\d 已回答/);
  await expect(ask.getByTestId("ask-question").first()).toContainText("备注：Retry-After 用秒数");
  // The answer is marked in the card only, not repeated elsewhere.
  await expect(page.getByTestId("transcript").getByText("先放内存，留好换 Redis 的接口")).toHaveCount(1);
  // Options with previews: the picked one's beside the options, another's while the pointer is on it.
  const layer = ask.getByTestId("ask-question").nth(2);
  const preview = layer.getByTestId("option-preview");
  await expect(preview).toHaveAttribute("data-option", "0");
  await expect(preview.locator("pre:not([aria-hidden])")).toContainText("auth");
  await layer.getByText("最外层").hover();
  await expect(preview).toHaveAttribute("data-option", "1");
  await expect(preview.locator("pre:not([aria-hidden])")).toContainText("按 IP 计数");
  // On the terminal's grid: a CJK character two columns, a box-drawing one and an ASCII one one each.
  const width = (text: string) =>
    preview
      .locator("pre:not([aria-hidden]) span")
      .filter({ hasText: new RegExp(`^${text}$`) })
      .first()
      .evaluate((e) => e.getBoundingClientRect().width);
  const col = await preview
    .locator("pre")
    .first()
    .evaluate((e) => {
      const probe = document.createElement("span");
      probe.textContent = "0".repeat(10);
      e.append(probe);
      const w = probe.getBoundingClientRect().width / 10;
      probe.remove();
      return w;
    });
  expect(await width("限")).toBeCloseTo(2 * col, 1);
  expect(await width("│")).toBeCloseTo(col, 1);
  const todo = page.locator('[data-tool="TodoWrite"]');
  await reveal(page, todo);
  await todo.getByRole("button").first().click();
  await expect(todo.getByTestId("checklist").locator("li")).toHaveCount(3);
});

test("AskUserQuestion 选择先聊聊时不显示为失败，标出已答的问题", async ({ page }) => {
  await openRich(page);
  const ask = page.getByTestId("ask").filter({ hasText: "按什么维度限流？" });
  await reveal(page, ask);
  await expect(ask).not.toHaveAttribute("data-failed");
  await expect(ask.getByTestId("ask-status")).toHaveText(/^你 · \d\d:\d\d 选择先聊聊$/);
  await expect(ask.locator("[data-chosen]")).toHaveText("API key");
  await expect(ask).not.toContainText("The user doesn't want to proceed");
});
