import type { CSSProperties } from "react";
import { useHighlight } from "./client";
import type { HlToken } from "./types";

export { highlight, langFromPath, useHighlight } from "./client";
export type { HlLines } from "./client";

/** Token spans of one line; colors follow `prefers-color-scheme` through `.shk` in index.css. */
export function TokenSpans({ tokens }: { tokens: HlToken[] }) {
  return (
    <>
      {tokens.map((t, i) =>
        t.l ? (
          <span key={i} className="shk" style={{ "--shiki-light": t.l, "--shiki-dark": t.d } as CSSProperties}>
            {t.c}
          </span>
        ) : (
          t.c
        ),
      )}
    </>
  );
}

/** Inline highlighted code (lines joined by "\n"); plain text until the worker answers. Used by markdown fences. */
export function HighlightedCode({ code, lang }: { code: string; lang: string | null }) {
  const lines = useHighlight(code, lang);
  if (!lines) return <>{code}</>;
  return (
    <>
      {lines.map((l, i) => (
        <span key={i}>
          {i > 0 && "\n"}
          <TokenSpans tokens={l} />
        </span>
      ))}
    </>
  );
}
