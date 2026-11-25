// Connect against the mock ccrpc (src/mocks/ccrpc.ts): each src/mocks/data/connect/*.jsonl is a
// recorded wire.jsonl replayed as one `ccrpc ls` entry, standing for a Session of the mock data:
// ui-cases → the rich Live Session, tools → the Flux Session, session-changed → the auth Session
// (its `/clear` moves to the JWT one), no-plugin → the LCP Live Session (plugin not reported),
// turns → the zod Session, trust → the review Session, refused → the Longhorn Session (its ccrpc exits
// before `ccrpcConnect` returns), clay-new → a Session not in the mock index (opened in clay, no prompt yet),
// resumed → the untitled Session (it ends right after connecting, as if `/exit` in the terminal),
// rewind → the Redis Session (listed only for the rewind tests: elsewhere it is the Session not in clay).

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

export const RICH = "为 /v1/chat 接口增加令牌桶限流中间件";

export const NOT_LISTED = "排查 Redis 连接池泄漏";

export const TURNS = "迁移表单校验到 zod";

export async function send(page: Page, text: string, mod?: "Meta") {
  const input = page.getByTestId("connect-input");
  await input.fill(text);
  await input.press(mod ? `${mod}+Enter` : "Enter");
}
