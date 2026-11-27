import { expect, test, type Page } from "@playwright/test";
import { openApp } from "./helpers";

const theme = (page: Page) => page.evaluate(() => document.documentElement.dataset.theme);
const ground = (page: Page) => page.evaluate(() => getComputedStyle(document.body).backgroundColor);

async function pickTheme(page: Page, value: "auto" | "light" | "dark") {
  await page.getByTestId("settings-button").click();
  await page.getByRole("tab", { name: "通用" }).click();
  await page.getByTestId(`theme-${value}`).click();
  await expect(page.getByTestId(`theme-${value}`)).toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("Escape");
}

type Lan = { enabled: boolean; addresses: string[]; error: string | null };
const setLan = (page: Page, lan: Lan) => page.evaluate((lan) => void (window.__cvCcrpc!.lan = lan), lan);

async function openGeneral(page: Page) {
  await page.goto("/");
  await page.getByTestId("settings-button").click();
  await page.getByRole("tab", { name: "通用" }).click();
  await expect(page.getByTestId("set-lan-access")).toBeVisible();
}

test("局域网访问：默认关闭，打开后显示地址，再关闭；切换失败写原因", async ({ page }) => {
  await openGeneral(page);
  const addresses = page.getByTestId("lan-addresses");
  await expect(page.getByTestId("lan-access-false")).toHaveAttribute("aria-checked", "true");
  await expect(page.getByTestId("set-lan-access")).toContainText(
    "开启后，配对过的设备可以经局域网连接这台 Mac 并操作 Session；关闭窗口不退出应用。",
  );
  await expect(addresses).toHaveCount(0);

  await page.getByTestId("lan-access-true").click();
  await expect(page.getByTestId("lan-access-true")).toHaveAttribute("aria-checked", "true");
  await expect(addresses).toHaveText("地址：192.168.2.50:38471");

  await page.getByTestId("lan-access-false").click();
  await expect(page.getByTestId("lan-access-false")).toHaveAttribute("aria-checked", "true");
  await expect(addresses).toHaveCount(0);

  await page.evaluate(() => void (window.__cvCcrpc!.lanFails = "写入设置失败"));
  await page.getByTestId("lan-access-true").click();
  await expect(page.getByTestId("lan-toggle-error")).toHaveText("写入设置失败");
  await expect(page.getByTestId("lan-access-false")).toHaveAttribute("aria-checked", "true");
  await page.getByTestId("lan-access-true").click();
  await expect(page.getByTestId("lan-toggle-error")).toHaveCount(0);
  await expect(addresses).toHaveText("地址：192.168.2.50:38471");
});

test("局域网访问：端口被占用时写明原因，之后的状态每 5 秒刷新；没有地址时写明", async ({ page }) => {
  await page.clock.install();
  await page.addInitScript((lan) => (window.__cvCcrpcPreset = { lan }), {
    enabled: true,
    addresses: [],
    error: "192.168.2.50:38471 已被占用",
  } as Lan);
  await openGeneral(page);
  await expect(page.getByTestId("lan-access-true")).toHaveAttribute("aria-checked", "true");
  await expect(page.getByTestId("lan-error")).toHaveText("192.168.2.50:38471 已被占用");
  await expect(page.getByTestId("lan-addresses")).toHaveCount(0);

  // The port was freed: picked up by the refresh, with the dialog left open.
  await setLan(page, { enabled: true, addresses: ["192.168.2.50:38471", "10.0.0.8:38471"], error: null });
  await page.clock.fastForward(5000);
  await expect(page.getByTestId("lan-addresses")).toHaveText("地址：192.168.2.50:38471、10.0.0.8:38471");
  await expect(page.getByTestId("lan-error")).toHaveCount(0);

  await setLan(page, { enabled: true, addresses: [], error: null });
  await page.clock.fastForward(5000);
  await expect(page.getByTestId("lan-addresses")).toHaveText("没有可用的局域网地址。");
});

test("局域网配对：扫码弹层显示二维码与倒计时，兑换后提示已配对并自动关闭、列表出现设备", async ({ page }) => {
  await openGeneral(page);
  await expect(page.getByTestId("lan-pair-button")).toHaveCount(0);
  await expect(page.getByTestId("lan-devices")).toHaveCount(0);

  await page.getByTestId("lan-access-true").click();
  await expect(page.getByTestId("lan-pair-button")).toBeVisible();

  await page.getByTestId("lan-pair-button").click();
  const modal = page.getByTestId("lan-pair-dialog");
  await expect(modal).toBeVisible();
  await expect(modal.getByTestId("lan-pair-qr")).toHaveAttribute("src", /^data:image\/svg\+xml;charset=utf-8,/);
  await expect(modal.getByTestId("lan-pair-url")).toContainText("codeck://pair?h=");
  await expect(modal.getByTestId("lan-pair-countdown")).toHaveText(/^[45]:\d{2} 后失效$/);

  await modal.getByTestId("lan-pair-copy").click();
  await expect(modal.getByTestId("lan-pair-copy")).toHaveText("已复制");

  const paired = await page.evaluate(() => window.__cvCcrpc!.lanRedeem("客厅的 iPhone"));
  await expect(modal.getByTestId("lan-pair-success")).toHaveText("已配对 客厅的 iPhone");
  await expect(modal).toHaveCount(0, { timeout: 5000 });

  // Paired offer must not be cancelled when the modal closes after success.
  expect(await page.evaluate(() => window.__cvCcrpc!.lanCancelled)).toEqual([]);

  const rows = page.getByTestId("lan-device-row");
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toContainText("客厅的 iPhone");
  await expect(rows.first()).toContainText(/配对于 \d{4}-\d{2}-\d{2} \d{2}:\d{2}/);
  await expect(rows.first()).toContainText("从未连接");

  // Once the device connects over WSS, the list refreshes and updates lastSeenAt.
  await page.evaluate((id) => window.__cvCcrpc!.lanTouchDevice(id), paired.id);
  await expect(rows.first()).not.toContainText("从未连接");
  await expect(rows.first()).toContainText(/最近连接 \S+/);
});

test("局域网配对：兑换事件早于 lanPairStart 回包到达仍显示已配对", async ({ page }) => {
  await openGeneral(page);
  await page.getByTestId("lan-access-true").click();
  await expect(page.getByTestId("lan-pair-button")).toBeVisible();

  await page.evaluate(() => void (window.__cvCcrpc!.lanPairEarlyRedeem = "早到的 iPhone"));
  await page.getByTestId("lan-pair-button").click();

  const modal = page.getByTestId("lan-pair-dialog");
  await expect(modal.getByTestId("lan-pair-success")).toHaveText("已配对 早到的 iPhone");
  await expect(modal).toHaveCount(0, { timeout: 5000 });
  expect(await page.evaluate(() => window.__cvCcrpc!.lanCancelled)).toEqual([]);

  const rows = page.getByTestId("lan-device-row");
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toContainText("早到的 iPhone");
  await expect(rows.first()).toContainText("从未连接");
});

test("局域网配对：关闭弹层取消当前报价；过期后显示已过期并可重新生成", async ({ page }) => {
  await openGeneral(page);
  await page.getByTestId("lan-access-true").click();
  await page.getByTestId("lan-pair-button").click();

  const modal = page.getByTestId("lan-pair-dialog");
  await expect(modal.getByTestId("lan-pair-qr")).toBeVisible();
  const firstId = await page.evaluate(() => window.__cvCcrpc!.lanOffer?.id);
  expect(firstId).toBeTruthy();

  await page.keyboard.press("Escape");
  await expect(modal).toHaveCount(0);
  await expect(page.getByTestId("settings")).toBeVisible();
  await expect.poll(() => page.evaluate(() => window.__cvCcrpc!.lanCancelled)).toEqual([firstId]);
  expect(await page.evaluate(() => window.__cvCcrpc!.lanOffer)).toBeNull();

  // Next offer expires quickly: shows expired state, refuses redemption, and lets the user regenerate a fresh one.
  await page.evaluate(() => void (window.__cvCcrpc!.lanOfferTtlMs = 350));
  await page.getByTestId("lan-pair-button").click();
  await expect(modal.getByTestId("lan-pair-qr")).toBeVisible();
  const shortId = await page.evaluate(() => window.__cvCcrpc!.lanOffer?.id);
  expect(shortId).not.toBe(firstId);

  await expect(modal.getByTestId("lan-pair-expired")).toHaveText("配对码已过期");
  await expect(modal.getByTestId("lan-pair-qr")).toHaveClass(/opacity-30/);

  await expect(page.evaluate(() => window.__cvCcrpc!.lanRedeem("过期设备"))).rejects.toThrow("配对码已过期");
  await expect(modal.getByTestId("lan-pair-success")).toHaveCount(0);
  await expect(modal.getByTestId("lan-pair-expired")).toHaveText("配对码已过期");
  expect(await page.evaluate(() => window.__cvCcrpc!.lanDevices)).toEqual([]);

  await modal.getByTestId("lan-pair-regenerate").click();
  await expect(modal.getByTestId("lan-pair-expired")).toHaveCount(0);
  await expect(modal.getByTestId("lan-pair-countdown")).toHaveText(/^[45]:\d{2} 后失效$/);
  const regenId = await page.evaluate(() => window.__cvCcrpc!.lanOffer?.id);
  expect(regenId).toBeTruthy();
  expect(regenId).not.toBe(shortId);

  await modal.getByTestId("lan-pair-close").click();
  await expect(modal).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => window.__cvCcrpc!.lanCancelled)).toEqual([firstId, regenId]);
});

test("局域网已配对设备：开关关着也列出已有设备，移除需确认", async ({ page }) => {
  const now = Date.now();
  await page.addInitScript(
    (lanDevices) => {
      window.__cvCcrpcPreset = { lanDevices };
    },
    [
      { id: "dev000000001", name: "工作 iPhone", pairedAt: now - 3 * 3_600_000, lastSeenAt: now - 5 * 60_000 },
      { id: "dev000000002", name: "备用 iPhone", pairedAt: now - 24 * 3_600_000, lastSeenAt: null },
    ],
  );
  await openGeneral(page);
  await expect(page.getByTestId("lan-access-false")).toHaveAttribute("aria-checked", "true");
  await expect(page.getByTestId("lan-pair-button")).toHaveCount(0);

  const rows = page.getByTestId("lan-device-row");
  await expect(rows).toHaveCount(2);
  await expect(rows.nth(0)).toContainText("工作 iPhone");
  await expect(rows.nth(0)).toContainText("最近连接 5 分钟前");
  await expect(rows.nth(1)).toContainText("备用 iPhone");
  await expect(rows.nth(1)).toContainText("从未连接");

  // Cancel keeps the row; confirming removes it.
  await rows.nth(0).getByTestId("lan-device-remove").click();
  await expect(rows.nth(0).getByTestId("lan-device-confirm-remove")).toBeVisible();
  await rows.nth(0).getByTestId("lan-device-cancel-remove").click();
  await expect(rows).toHaveCount(2);

  await rows.nth(0).getByTestId("lan-device-remove").click();
  await rows.nth(0).getByTestId("lan-device-confirm-remove").click();
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toContainText("备用 iPhone");
});

test("局域网已配对设备：首次读取延迟返回旧列表期间收到事件，最终显示新列表", async ({ page }) => {
  const now = Date.now();
  await page.addInitScript(
    (lanDevices) => {
      window.__cvCcrpcPreset = { lanDevices, holdLanDevices: true };
    },
    [{ id: "dev000000001", name: "旧 iPhone", pairedAt: now - 3_600_000, lastSeenAt: null }],
  );
  await openGeneral(page);

  // Wait until both the mount fetch and post-subscribe refresh are held with the stale snapshot,
  // while TanStack Query still has no cached data.
  await expect.poll(() => page.evaluate(() => window.__cvCcrpc!.heldLanDevices)).toBe(2);
  await expect(page.getByTestId("lan-devices")).toHaveCount(0);

  // Allow subsequent reads to succeed immediately, mutate the device list, and emit the event
  // while the initial stale reads are still in flight.
  await page.evaluate(() => {
    const p = window.__cvCcrpc!;
    p.holdLanDevices = false;
    p.lanDevices = [...p.lanDevices, { id: "dev000000002", name: "新 iPhone", pairedAt: Date.now(), lastSeenAt: null }];
    p.lanTouchDevice("dev000000001");
  });

  const rows = page.getByTestId("lan-device-row");
  await expect(rows).toHaveCount(2);
  await expect(rows.nth(0)).toContainText("旧 iPhone");
  await expect(rows.nth(0)).toContainText(/最近连接 \S+/);
  await expect(rows.nth(1)).toContainText("新 iPhone");

  // Releasing the delayed stale fetches afterwards must not clobber the newer list.
  await page.evaluate(async () => {
    window.__cvCcrpc!.releaseLanDevices();
    await new Promise((r) => setTimeout(r, 50));
  });
  await expect(rows).toHaveCount(2);
  await expect(rows.nth(0)).toContainText(/最近连接 \S+/);
  await expect(rows.nth(1)).toContainText("新 iPhone");
});

test("局域网已配对设备：订阅建立前漏掉的事件在订阅完成后补刷出来", async ({ page }) => {
  const now = Date.now();
  await page.addInitScript(
    (lanDevices) => {
      window.__cvCcrpcPreset = { lanDevices, holdLanSubscribe: true };
    },
    [{ id: "dev000000001", name: "旧 iPhone", pairedAt: now - 3_600_000, lastSeenAt: null }],
  );
  await openGeneral(page);

  const rows = page.getByTestId("lan-device-row");
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toContainText("从未连接");

  // Event fires before `onLanDevices` finishes registering, so the listener misses it.
  await page.evaluate(() => {
    const p = window.__cvCcrpc!;
    p.lanDevices = [
      ...p.lanDevices,
      { id: "dev000000002", name: "订阅前配对的 iPhone", pairedAt: Date.now(), lastSeenAt: null },
    ];
    p.lanTouchDevice("dev000000001");
  });
  await expect(rows).toHaveCount(1);

  // Once subscription completes, the post-subscribe refresh picks up the missed update.
  await page.evaluate(() => window.__cvCcrpc!.releaseLanSubscribe());
  await expect(rows).toHaveCount(2);
  await expect(rows.nth(0)).toContainText(/最近连接 \S+/);
  await expect(rows.nth(1)).toContainText("订阅前配对的 iPhone");
});

test("局域网配对：订阅失败后重新生成先重连订阅，持续失败显示原因，恢复后可完成配对", async ({ page }) => {
  await openGeneral(page);
  await page.getByTestId("lan-access-true").click();
  await expect(page.getByTestId("lan-pair-button")).toBeVisible();

  await page.evaluate(() => void (window.__cvCcrpc!.lanSubscribeFails = "订阅设备事件失败"));
  await page.getByTestId("lan-pair-button").click();

  const modal = page.getByTestId("lan-pair-dialog");
  await expect(modal.getByTestId("lan-pair-error")).toHaveText("订阅设备事件失败");
  await expect(modal.getByTestId("lan-pair-qr")).toHaveCount(0);
  expect(await page.evaluate(() => window.__cvCcrpc!.lanOffer)).toBeNull();

  // Retry while subscription still fails: must not call `lanPairStart`, and shows the latest error.
  await page.evaluate(() => void (window.__cvCcrpc!.lanSubscribeFails = "事件通道仍不可用"));
  await modal.getByTestId("lan-pair-regenerate").click();
  await expect(modal.getByTestId("lan-pair-error")).toHaveText("事件通道仍不可用");
  await expect(modal.getByTestId("lan-pair-qr")).toHaveCount(0);
  expect(await page.evaluate(() => window.__cvCcrpc!.lanOffer)).toBeNull();

  // Subscription recovers: regenerate subscribes first, then fetches offer and receives the redeem event.
  await page.evaluate(() => void (window.__cvCcrpc!.lanSubscribeFails = undefined));
  await modal.getByTestId("lan-pair-regenerate").click();
  await expect(modal.getByTestId("lan-pair-error")).toHaveCount(0);
  await expect(modal.getByTestId("lan-pair-qr")).toBeVisible();

  await page.evaluate(() => window.__cvCcrpc!.lanRedeem("恢复后的 iPhone"));
  await expect(modal.getByTestId("lan-pair-success")).toHaveText("已配对 恢复后的 iPhone");
  await expect(modal).toHaveCount(0, { timeout: 5000 });
  await expect(page.getByTestId("lan-device-row").first()).toContainText("恢复后的 iPhone");
});

test("界面主题：自动跟随系统，浅色 / 暗色固定，选择会被记住", async ({ page }) => {
  await page.emulateMedia({ colorScheme: "dark" });
  await page.goto("/");
  // 自动 by default: the system's dark.
  expect(await theme(page)).toBe("dark");
  expect(await ground(page)).toBe("rgb(30, 30, 32)");
  await pickTheme(page, "light");
  expect(await theme(page)).toBe("light");
  expect(await ground(page)).toBe("rgb(255, 255, 255)");
  // Kept across launches, applied before the app starts.
  await page.reload();
  expect(await theme(page)).toBe("light");
  // The system changing does not move a fixed theme.
  await page.emulateMedia({ colorScheme: "light" });
  await pickTheme(page, "dark");
  expect(await ground(page)).toBe("rgb(30, 30, 32)");
  // Back to 自动: the system's again, and following it.
  await pickTheme(page, "auto");
  expect(await theme(page)).toBe("light");
  await page.emulateMedia({ colorScheme: "dark" });
  await expect.poll(() => theme(page)).toBe("dark");
});

test("⌘, 打开设置，格式兼容性渲染 mock 计数", async ({ page }) => {
  await openApp(page);
  await page.keyboard.press("Meta+,");
  const dialog = page.getByRole("dialog");
  await expect(dialog).toBeVisible();
  await expect(dialog.getByTestId("set-version")).toContainText("0.1.0");

  await dialog.getByRole("tab", { name: "格式兼容性" }).click();
  await expect(dialog.getByTestId("diag-card-failed")).toContainText("3 行");
  await expect(dialog.getByTestId("diag-card-tools")).toContainText("1 个");
  await expect(dialog.getByTestId("diag-entry-types")).toContainText("frame-link");
  await expect(dialog.getByTestId("diag-tools")).toContainText("ScheduleWakeup");
  // A user message with a structure mark this version does not recognize.
  await expect(dialog.getByTestId("diag-messages")).toContainText("<handoff-message>");
  // Each card counts the tables of its own kind: entry / block / subtype for types, messages apart.
  await expect(dialog.getByTestId("diag-card-types")).toContainText("3 种");
  await expect(dialog.getByTestId("diag-card-messages")).toContainText("1 种");
  await expect(dialog.getByTestId("diag-version-row").locator("code")).toHaveText(["2.1.91", "2.1.88"]);
  await expect(dialog.getByTestId("diag-failures")).toContainText("5e1c0b7a-0006");
  await expect(dialog.getByTestId("diag-integrity")).toContainText("2");
  await page.screenshot({ path: process.env.SHOT ?? test.info().outputPath("diagnostics.png") });

  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
});

test("更改数据目录调用 API 并更新显示，恢复默认；应用后清空已选 Session", async ({ page }) => {
  await openApp(page);
  await page.locator("[data-testid=session-row]:not([data-pending])").first().click();
  await expect(page.getByTestId("transcript")).toBeVisible();
  await page.keyboard.press("Meta+,");
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("tab", { name: "数据" }).click();
  await expect(dialog.getByTestId("set-root-path")).toHaveText("/Users/dev/.claude");
  await expect(dialog.getByTestId("set-root-source")).toContainText("默认");

  await dialog.getByTestId("set-root-input").fill("/tmp/fixture-root");
  await dialog.getByTestId("set-root-apply").click();
  await expect(dialog.getByTestId("set-root-path")).toHaveText("/tmp/fixture-root");
  await expect(dialog.getByTestId("set-root-source")).toContainText("设置");

  await page.keyboard.press("Escape");
  await expect(page.getByTestId("transcript-empty")).toBeVisible();
  await page.keyboard.press("Meta+,");
  await dialog.getByRole("tab", { name: "数据" }).click();

  await dialog.getByTestId("set-root-reset").click();
  await expect(dialog.getByTestId("set-root-source")).toContainText("默认");

  await dialog.getByTestId("set-rebuild").click();
  await expect(dialog.getByTestId("set-index-status")).toContainText("已索引");
});
