import { expect, test } from "@playwright/test";
import { disableAutoConnect, probe } from "./connect-helpers";
import { openApp } from "./helpers";

// Connected by hand here (「Connect」 would race 自动 Connect); 自动 Connect has a test of its own.
test.beforeEach(({ page }) => disableAutoConnect(page));

test("设置 › ccrpc：ccrpc 路径、doctor 的失败项、日志目录；分析器的 Key、Endpoint、模型可在这里设置", async ({
  page,
}) => {
  await openApp(page);
  await page.keyboard.press("Meta+,");
  await page.getByRole("tab", { name: "ccrpc" }).click();
  await expect(page.getByTestId("set-ccrpc")).toContainText("/mock/bin/ccrpc");
  await expect(page.getByTestId("set-ccrpc")).toContainText("CODECK_CCRPC");
  await expect(page.getByTestId("set-doctor").locator('[data-level="fail"]')).toContainText("ccrpc 插件未启用");
  await expect(page.getByTestId("set-ccrpc-logs")).toContainText("/mock/Logs/dev.joy.codeck/ccrpc");

  // The analyzer: a key from the login shell until one is saved here, with an endpoint and model.
  const status = page.getByTestId("analyzer-status");
  await expect(status).toHaveText("已开启 · Key 来自登录 shell 的 CCRPC_ANALYZER_API_KEY（…abcd）");
  await expect(page.getByTestId("analyzer-base-url")).toHaveAttribute(
    "placeholder",
    "https://api.deepseek.com/anthropic",
  );
  await page.getByTestId("analyzer-key").fill("sk-ant-test-9876");
  await page.getByTestId("analyzer-base-url").fill("https://api.anthropic.com");
  await page.getByTestId("analyzer-model").fill("claude-haiku-4-5");
  await page.getByTestId("analyzer-save").click();
  await expect(status).toHaveText("已开启 · Key 保存在这里（…9876）");
  await expect(page.getByTestId("analyzer-key")).toHaveValue("");
  await expect(page.getByTestId("analyzer-base-url")).toHaveValue("https://api.anthropic.com");
  await expect(page.getByTestId("set-analyzer")).toContainText("之后新建的连接生效");
  // Saving again with the key box empty keeps the key; clearing it goes back to the shell's.
  await page.getByTestId("analyzer-save").click();
  await expect(status).toHaveText("已开启 · Key 保存在这里（…9876）");
  await page.getByTestId("analyzer-clear-key").click();
  await expect(status).toContainText("来自登录 shell");
  expect((await probe(page)).analyzerSaves).toBe(3);

  // 「测试」 tries the values in the form without saving them: the model typed here, a key typed here.
  const box = page.getByTestId("set-analyzer");
  await page.getByTestId("analyzer-test").click();
  await expect(box).toContainText("可用：claude-haiku-4-5 用时 2.3 秒");
  await page.getByTestId("analyzer-key").fill("sk-bad");
  await page.getByTestId("analyzer-test").click();
  await expect(box).toContainText("不可用：分析器请求失败：http status: 401");
  await expect(page.getByTestId("analyzer-key")).toHaveValue("sk-bad");
  await expect(status).toContainText("来自登录 shell");
  expect((await probe(page)).analyzerSaves).toBe(3);
});

test("设置 › ccrpc：分析记录从新到旧列出学会的对话框与失败原因，可打开记录目录", async ({ page }) => {
  await openApp(page);
  await page.keyboard.press("Meta+,");
  await page.getByRole("tab", { name: "ccrpc" }).click();
  const rows = page.getByTestId("analyzer-record");
  await expect(rows).toHaveCount(2);
  await expect(rows.nth(0)).toContainText("学会");
  await expect(rows.nth(0)).toContainText("Computer Use wants to control these apps");
  await expect(rows.nth(1)).toContainText("失败");
  await expect(rows.nth(1)).toContainText("分析器认为屏幕上没有等待作答的菜单");
  await expect(page.getByTestId("set-analyzer-records")).toContainText("/mock/.local/share/ccrpc/analyses");
  await page.getByTestId("analyzer-records-open").click();
  await expect.poll(async () => (await probe(page)).recordsOpened).toBe(1);
});

test("设置 › ccrpc › 启动参数：选目录、填参数、保存；参数为空的目录保存时去掉，可删除", async ({ page }) => {
  await openApp(page);
  await page.keyboard.press("Meta+,");
  await page.getByRole("tab", { name: "ccrpc" }).click();
  const box = page.getByTestId("set-launch-rules");
  await expect(box.getByTestId("launch-rule")).toHaveCount(0);
  await box.getByTestId("launch-rule-add").click();
  await expect(box.getByTestId("launch-rule")).toContainText("/tmp/mock-new");
  // The picker returning a directory already listed adds nothing.
  await box.getByTestId("launch-rule-add").click();
  await expect(box.getByTestId("launch-rule")).toHaveCount(1);
  await box.getByTestId("launch-rule-args").fill("--add-dir=/tmp/extra");
  await box.getByTestId("launch-rules-save").click();
  await expect(box).toContainText("已保存，之后新建的 Session 生效");

  // Saved: back after reopening the settings.
  await page.keyboard.press("Escape");
  await page.keyboard.press("Meta+,");
  await page.getByRole("tab", { name: "ccrpc" }).click();
  await expect(box.getByTestId("launch-rule-args")).toHaveValue("--add-dir=/tmp/extra");

  // Blank arguments drop the rule on saving; removing it does too.
  await box.getByTestId("launch-rule-args").fill("  ");
  await box.getByTestId("launch-rules-save").click();
  await expect(box.getByTestId("launch-rule")).toHaveCount(0);
  await box.getByTestId("launch-rule-add").click();
  await box.getByTestId("launch-rule-remove").click();
  await expect(box.getByTestId("launch-rule")).toHaveCount(0);
});

test("设置 › ccrpc › 新开的 Pane：默认显示，选「隐藏」即保存", async ({ page }) => {
  await openApp(page);
  await page.keyboard.press("Meta+,");
  await page.getByRole("tab", { name: "ccrpc" }).click();
  const box = page.getByTestId("set-hide-new-panes");
  await expect(box.getByTestId("hide-new-panes-show")).toHaveAttribute("aria-checked", "true");
  await box.getByTestId("hide-new-panes-hide").click();
  await expect(box.getByTestId("hide-new-panes-hide")).toHaveAttribute("aria-checked", "true");
  await expect(box.getByTestId("hide-new-panes-show")).toHaveAttribute("aria-checked", "false");
  expect((await probe(page)).hideNewPanes).toBe(true);
});
