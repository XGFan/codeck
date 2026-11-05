import { expect, type Locator, type Page } from "@playwright/test";
import { unfoldAll } from "./unfold";

/** The mock Session with the most structure (Subagents, tool groups, Branches, live). */
export const RICH_TITLE = "为 /v1/chat 接口增加令牌桶限流中间件";

/**
 * Opens a Session with every turn's process unfolded (most tests look inside it); `folded` leaves it as it opens.
 * Returns its id.
 */
export async function openSession(page: Page, title: string, { folded = false } = {}) {
  await page.goto("/");
  const row = page.getByTestId("session-row").filter({ hasText: title });
  await row.click();
  await expect(page.getByTestId("header").getByRole("heading", { name: title })).toBeVisible();
  if (!folded) await unfoldAll(page);
  return (await row.getAttribute("data-session-id"))!;
}

export const openRich = (page: Page, opts?: { folded?: boolean }) => openSession(page, RICH_TITLE, opts);

/** Scrolls `scroller` by `dy` and resolves after the scroll event (and the virtualizer's re-render); false at the end. */
export const scrollBy = (scroller: Locator, dy: number) =>
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

/**
 * Scrolls a virtualized list until `target` is rendered: up from the bottom (`from: "bottom"`), or down from the top
 * opening collapsed tool groups on the way (`from: "top"`).
 */
export async function reveal(scroller: Locator, target: Locator, from: "bottom" | "top" = "bottom") {
  await expect(scroller.locator("[data-row]").first()).toBeVisible();
  if (from === "bottom") {
    await scrollBy(scroller, 1e7);
    for (let i = 0; i < 40 && (await target.count()) === 0; i++) {
      if (!(await scrollBy(scroller, -300))) break;
    }
  } else {
    // Resolve only once the scroll event has fired: the virtualizer re-renders in its scroll handler, so until
    // then the rows of the previous position (the session opens at the bottom) are still mounted and
    // `target.count()` would match a row that unmounts right after.
    await scroller.evaluate(
      (e) =>
        new Promise<void>((done) => {
          if (e.scrollTop === 0) return done();
          e.addEventListener("scroll", () => done(), { once: true });
          e.scrollTop = 0;
        }),
    );
    for (let i = 0; i < 200 && (await target.count()) === 0; i++) {
      const closed = scroller.locator('[data-testid="tool-group"]:not([data-open]) > button');
      if ((await closed.count()) > 0) {
        const button = (await closed.first().elementHandle())!;
        await button.click();
        await expect.poll(() => button.getAttribute("aria-expanded")).toBe("true");
      } else if (!(await scrollBy(scroller, 400))) break;
    }
  }
  await target.first().scrollIntoViewIfNeeded();
}
