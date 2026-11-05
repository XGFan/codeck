import type { Locator, Page } from "@playwright/test";

/** Simulates an IME (e.g. pinyin) composition: compositionstart, then one composing `input` per step. */
export async function composeStart(input: Locator, steps: string[]) {
  await input.evaluate((el, steps) => {
    const field = el as HTMLInputElement;
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    field.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" }));
    for (const s of steps) {
      setValue.call(field, s);
      field.dispatchEvent(
        new InputEvent("input", { bubbles: true, isComposing: true, inputType: "insertCompositionText", data: s }),
      );
      field.dispatchEvent(new CompositionEvent("compositionupdate", { bubbles: true, data: s }));
    }
  }, steps);
}

/** Commits the composition with `text` (spec order: the last composing `input`, then compositionend). */
export async function composeEnd(input: Locator, text: string) {
  await input.evaluate((el, text) => {
    const field = el as HTMLInputElement;
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    setValue.call(field, text);
    field.dispatchEvent(
      new InputEvent("input", { bubbles: true, isComposing: true, inputType: "insertFromComposition", data: text }),
    );
    field.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: text }));
  }, text);
}

/** Esc pressed while composing, as WebKit delivers it (the IME consumes it to cancel the candidates). */
export async function escWhileComposing(input: Locator) {
  await input.evaluate((el) => {
    el.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Escape",
        code: "Escape",
        isComposing: true,
        bubbles: true,
        cancelable: true,
      }),
    );
  });
}

/** Queries passed to the mock `search` / `findInSession`. */
export const queriesOf = (page: Page, method: "search" | "findInSession") =>
  page.evaluate((m) => (window.__cvCalls ?? []).filter((c) => c.method === m).map((c) => c.args[0] as string), method);
