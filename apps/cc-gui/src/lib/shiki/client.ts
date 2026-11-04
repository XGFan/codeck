import { useEffect, useState } from "react";
import type { HlRequest, HlResponse, HlToken } from "./types";

export type HlLines = HlToken[][];

/** Beyond this the highlight is skipped and plain text is shown. */
export const MAX_HIGHLIGHT_CHARS = 150_000;
const MAX_LINES = 3000;

const EXT: Record<string, string> = {
  rs: "rust", ts: "typescript", tsx: "tsx", js: "javascript", jsx: "jsx", mjs: "javascript", cjs: "javascript",
  json: "json", jsonc: "jsonc", toml: "toml", yaml: "yaml", yml: "yaml", md: "markdown", mdx: "mdx", py: "python",
  go: "go", java: "java", kt: "kotlin", swift: "swift", rb: "ruby", php: "php", c: "c", h: "c", cc: "cpp", cpp: "cpp",
  hpp: "cpp", cs: "csharp", sh: "bash", bash: "bash", zsh: "bash", fish: "fish", css: "css", scss: "scss",
  html: "html", vue: "vue", svelte: "svelte", sql: "sql", xml: "xml", conf: "nginx", ini: "ini", lua: "lua",
  dockerfile: "docker", tf: "terraform", graphql: "graphql", proto: "proto", diff: "diff", lock: "toml",
};

/** Shiki language id from a file path (by extension / well-known names), or null. */
export function langFromPath(path: string): string | null {
  const name = path.split("/").pop()?.toLowerCase() ?? "";
  if (name === "dockerfile") return "docker";
  if (name === "makefile") return "make";
  const ext = name.includes(".") ? name.split(".").pop()! : "";
  return EXT[ext] ?? null;
}

let worker: Worker | null | undefined;
let seq = 0;
const pending = new Map<number, (l: HlLines | null) => void>();
const cache = new Map<string, HlLines | null>();

function getWorker(): Worker | null {
  if (worker !== undefined) return worker;
  try {
    worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
    worker.onmessage = (ev: MessageEvent<HlResponse>) => {
      pending.get(ev.data.id)?.(ev.data.lines);
      pending.delete(ev.data.id);
    };
    worker.onerror = () => {
      for (const done of pending.values()) done(null);
      pending.clear();
    };
  } catch {
    worker = null;
  }
  return worker;
}

/** Highlights `code` in the Shiki worker; resolves null when unsupported / too large / failed. */
export function highlight(code: string, lang: string): Promise<HlLines | null> {
  if (code.length > MAX_HIGHLIGHT_CHARS || code.split("\n", MAX_LINES + 1).length > MAX_LINES) return Promise.resolve(null);
  const key = `${lang}\u0000${code}`;
  if (cache.has(key)) return Promise.resolve(cache.get(key)!);
  const w = getWorker();
  if (!w) return Promise.resolve(null);
  return new Promise((resolve) => {
    const id = ++seq;
    pending.set(id, (lines) => {
      if (cache.size > 200) cache.delete(cache.keys().next().value!);
      cache.set(key, lines);
      resolve(lines);
    });
    w.postMessage({ id, code, lang } satisfies HlRequest);
  });
}

/** Highlighted lines for `code`, or null while loading / when plain text should be shown. */
export function useHighlight(code: string, lang: string | null): HlLines | null {
  const [res, setRes] = useState<{ key: string; lines: HlLines | null } | null>(null);
  const key = lang ? `${lang}\u0000${code}` : "";
  useEffect(() => {
    if (!lang) return;
    let live = true;
    void highlight(code, lang).then((lines) => live && setRes({ key, lines }));
    return () => {
      live = false;
    };
  }, [code, lang, key]);
  return res && res.key === key ? res.lines : null;
}
