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
