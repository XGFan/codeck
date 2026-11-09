import { expect, test } from "@playwright/test";

test("两栏布局与统计、设置入口可见", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("pane-sessions")).toBeVisible();
  await expect(page.getByTestId("pane-conversation")).toBeVisible();
  await expect(page.getByTestId("nav-stats")).toBeVisible();
  await expect(page.getByTestId("settings-button")).toBeVisible();
  // No bar under the conversation: stats and settings are at the foot of the Session list, without the index count
  // (in Settings; here it changed while typing)…
  const sessions = page.getByTestId("pane-sessions");
  await expect(sessions.getByTestId("statusbar")).toBeVisible();
  await expect(sessions.getByTestId("statusbar")).not.toContainText("已索引");
  await expect(sessions.getByTestId("nav-stats")).toBeVisible();
  await expect(page.getByTestId("pane-conversation").getByTestId("settings-button")).toHaveCount(0);
  // In the window's bottom-left corner, settings first.
  const pane = (await sessions.boundingBox())!;
  const settingsBox = (await sessions.getByTestId("settings-button").boundingBox())!;
  const statsBox = (await sessions.getByTestId("nav-stats").boundingBox())!;
  expect(settingsBox.x - pane.x).toBeLessThan(10);
  expect(settingsBox.x).toBeLessThan(statsBox.x);
  // …and join the toolbar while the list is hidden.
  await page.keyboard.press("Control+Meta+L");
  await expect(sessions).toBeHidden();
  await expect(page.getByTestId("toolbar").getByTestId("nav-stats")).toBeVisible();
  await expect(page.getByTestId("toolbar").getByTestId("settings-button")).toBeVisible();
  const toolbarSettings = (await page.getByTestId("toolbar").getByTestId("settings-button").boundingBox())!;
  const toolbarStats = (await page.getByTestId("toolbar").getByTestId("nav-stats").boundingBox())!;
  expect(toolbarSettings.x).toBeLessThan(toolbarStats.x);
  await expect(page.getByTestId("statusbar")).toHaveCount(0);
});

test("索引状态到达时按到一半的设置按钮仍然有效", async ({ page }) => {
  // Paused clock: the app renders, but no IPC reply (the mock answers on a timer) arrives until it runs.
  await page.clock.install();
  await page.clock.pauseAt(Date.now() + 1000);
  await page.goto("/");
  const button = page.getByTestId("settings-button");
  await expect(button).toBeVisible();
  await expect(page.getByTestId("statusbar")).toHaveCount(0);
  const box = (await button.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  // The index status arrives between the press and the release.
  await page.clock.runFor(1000);
  await expect(page.getByTestId("statusbar")).toHaveCount(1);
  await page.mouse.up();
  await expect(page.getByTestId("settings")).toBeVisible();
});

test("右侧对话头部不再有搜索按钮", async ({ page }) => {
  await page.goto("/");
  await page.locator("[data-testid=session-row]:not([data-pending])").first().click();
  await expect(page.getByTestId("header")).toBeVisible();
  await expect(page.getByTestId("header")).not.toContainText("搜索");
  await expect(page.getByTestId("search-button")).toBeVisible();
});
