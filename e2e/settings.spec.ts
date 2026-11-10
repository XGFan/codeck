import { expect, test, type Page } from "@playwright/test";

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
    "开启后，同一网络的设备无需验证即可连接这台 Mac 并操作 Session；关闭窗口不退出应用。",
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
  await expect(page.getByTestId("lan-addresses")).toHaveText("地址：192.168.2.50:38471、10.0.0.8:38471", {
    timeout: 7000,
  });
  await expect(page.getByTestId("lan-error")).toHaveCount(0);

  await setLan(page, { enabled: true, addresses: [], error: null });
  await expect(page.getByTestId("lan-addresses")).toHaveText("没有可用的局域网地址。", { timeout: 7000 });
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
