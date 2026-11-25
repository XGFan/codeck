import { expect, type Locator, type Page } from "@playwright/test";

export const TOOLS = "为 Flux 增加 kestrel-cli 镜像自动更新";

export type Probe = {
  sent: { msg: Record<string, unknown> }[];
  watchStarts: number;
  watching: boolean;
  guiLog: string[];
  badge: number;
  analyzerSaves: number;
  recordsOpened: number;
  paneHides: { holderId: string; hidden: boolean }[];
  hideNewPanes: boolean;
};

export const probe = (page: Page) => page.evaluate(() => window.__cvCcrpc as unknown as Probe);

export const lastSent = async (page: Page, type: string) =>
  (await probe(page)).sent
    .map((s) => s.msg)
    .filter((m) => m.type === type)
    .pop();

export async function open(page: Page, title: string) {
  await page.getByTestId("session-row").filter({ hasText: title }).click();
  await expect(page.getByTestId("header").getByRole("heading", { name: title })).toBeVisible();
}

/** The header's 「···」 menu, opened: Connect, Disconnect and the Pane are in it. */
export async function headerMenu(page: Page) {
  const menu = page.getByTestId("header-menu");
  if (!(await menu.isVisible())) await page.getByTestId("header-more").click();
  return menu;
}

/** Clicks an item of the header's 「···」 menu. */
export const pick = async (page: Page, testId: string) => (await headerMenu(page)).getByTestId(testId).click();

/** Checks an item of the header's 「···」 menu, then closes the menu. */
export async function inMenu(page: Page, testId: string, check: (item: Locator) => Promise<void>) {
  await check((await headerMenu(page)).getByTestId(testId));
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("header-menu")).toHaveCount(0);
}

export async function connectTo(page: Page, title: string) {
  await page.goto("/");
  await open(page, title);
  await pick(page, "connect-button");
  await expect(page.getByTestId("connect-panel")).toBeVisible();
}

export const state = (page: Page) => page.getByTestId("connect-state");

/** Disables auto-connect before navigation so tests can click 「Connect」 without racing it. */
export const disableAutoConnect = (page: Page) =>
  page.addInitScript(
    () => localStorage.getItem("codeck.autoConnect") ?? localStorage.setItem("codeck.autoConnect", '{"on":false}'),
  );
