import type { Page } from "@playwright/test";

/**
 * Opens every turn's folded process in the transcript (top to bottom; rows are virtualized, so it scrolls), for
 * tests of what the process holds. Ends at the bottom.
 */
export async function unfoldAll(page: Page) {
  const t = page.getByTestId("transcript");
  await t.locator("[data-row]").first().waitFor();
  await t.evaluate((el) => (el.scrollTop = 0));
  for (let i = 0; i < 200; i++) {
    const closed = t.locator('[data-testid="turn-fold"][aria-expanded="false"]');
    if (await closed.count()) {
      await closed.first().click();
      continue;
    }
    const moved = await t.evaluate(
      (el) =>
        new Promise<boolean>((done) => {
          const before = el.scrollTop;
          el.scrollTop += el.clientHeight * 0.7;
          if (el.scrollTop === before) return done(false);
          el.addEventListener("scroll", () => requestAnimationFrame(() => done(true)), { once: true });
        }),
    );
    if (!moved) break;
  }
}
