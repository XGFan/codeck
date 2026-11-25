/// <reference lib="webworker" />
import { createHighlighterCore, type HighlighterCore } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";
import { bundledLanguages } from "shiki/langs";
import { bundledThemes } from "shiki/themes";
import type { HlRequest, HlResponse, HlToken } from "./types";

const LIGHT = "github-light";
const DARK = "github-dark";

let hl: Promise<HighlighterCore> | null = null;
const loaded = new Set<string>();

const highlighter = () =>
  (hl ??= createHighlighterCore({
    themes: [bundledThemes[LIGHT], bundledThemes[DARK]],
    langs: [],
    engine: createJavaScriptRegexEngine(),
  }));

self.onmessage = async (ev: MessageEvent<HlRequest>) => {
  const { id, code, lang } = ev.data;
  const reply = (lines: HlToken[][] | null) => self.postMessage({ id, lines } satisfies HlResponse);
  try {
    const loader = (bundledLanguages as Record<string, unknown>)[lang];
    if (!loader) return reply(null);
    const h = await highlighter();
    if (!loaded.has(lang)) {
      await h.loadLanguage(loader as never);
      loaded.add(lang);
    }
    const r = h.codeToTokens(code, { lang, themes: { light: LIGHT, dark: DARK }, defaultColor: false });
    reply(
      r.tokens.map((line) =>
        line.map((t) => ({ c: t.content, l: t.htmlStyle?.["--shiki-light"], d: t.htmlStyle?.["--shiki-dark"] })),
      ),
    );
  } catch {
    reply(null);
  }
};
