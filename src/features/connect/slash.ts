import type { SlashCommand } from "@/ipc/bindings";

/** The command name being typed (without the slash): the text is `/` and a name, no space or line break yet. */
export function typedCommand(text: string): string | null {
  const m = /^\/([^\s/]*)$/.exec(text);
  return m ? m[1]! : null;
}

/**
 * Commands for `prefix`, best first: the name itself, names starting with it, then a namespaced name
 * whose part after `:` starts with it, then names containing it; the list's order within each.
 */
export function matchCommands(list: SlashCommand[], prefix: string, limit = Infinity): SlashCommand[] {
  const p = prefix.toLowerCase();
  const rank = (c: SlashCommand) => {
    const n = c.name.toLowerCase();
    if (n === p) return 0;
    if (n.startsWith(p)) return 1;
    if (n.split(":").pop()!.startsWith(p)) return 2;
    return n.includes(p) ? 3 : -1;
  };
  return list
    .map((c, i) => ({ c, i, r: rank(c) }))
    .filter((x) => x.r >= 0)
    .sort((a, b) => a.r - b.r || a.i - b.i)
    .slice(0, limit)
    .map((x) => x.c);
}

/** The name typed at the start of `body` (`/name args`), without the slash; empty when `body` is no `/name`. */
export const typedName = (body: string) => (body.startsWith("/") ? body.slice(1).split(/\s/, 1)[0]! : "");

/**
 * The command named at the start of `body` (`/name args`), when it is one in `list`. As in Claude Code, a plugin's
 * command may be named without its plugin (`/autopilot` for `oh-my-claudecode:autopilot`) when no other has that name.
 */
export function commandOf(body: string, list: SlashCommand[] | undefined): SlashCommand | null {
  const name = typedName(body);
  if (!name || !list) return null;
  const exact = list.find((c) => c.name === name);
  if (exact) return exact;
  if (name.includes(":")) return null;
  const short = list.filter((c) => c.source === "plugin" && c.name.split(":").pop() === name);
  return short.length === 1 ? short[0]! : null;
}

/**
 * What ccrpc's `slash` should type for `body`, or null to send it as a `prompt`. Built-in commands
 * (and command-like names not in the loaded list) go as `slash`: a local command or a panel leaves
 * nothing in the transcript a `prompt` could wait for. Custom commands and skills, a path such as
 * `/Users/…`, a multi-line text, one with images, or anything while the list is unknown go as `prompt`.
 */
export function slashCommand(body: string, withImages: boolean, list: SlashCommand[] | undefined): string | null {
  if (!list || !body.startsWith("/") || withImages || body.includes("\n")) return null;
  const name = body.slice(1).split(/\s/, 1)[0]!;
  if (!/^[\w.:-]+$/.test(name)) return null;
  const known = commandOf(body, list);
  return known && known.source !== "builtin" ? null : body.slice(1);
}

/** A path being typed: `partial` spans `start`–`end` of the text; `files` when files are offered too. */
export interface PathAt {
  partial: string;
  start: number;
  end: number;
  files: boolean;
}

/**
 * The path to complete with the caret at `caret`: the whole argument of `/add-dir <path>` (directories), or the
 * `@path` word the caret is in (files too; `@` at the start or after white space, the word up to the next white space).
 */
export function pathAt(text: string, caret: number): PathAt | null {
  const dir = /^\/add-dir +([^\n]*)$/.exec(text);
  if (dir) return { partial: dir[1]!, start: text.length - dir[1]!.length, end: text.length, files: false };
  const at = /(?:^|\s)@([^\s@]*)$/.exec(text.slice(0, caret));
  if (!at) return null;
  const rest = /^[^\s]*/.exec(text.slice(caret))![0];
  return { partial: at[1]! + rest, start: caret - at[1]!.length, end: caret + rest.length, files: true };
}

/** `text` with the path at `p` replaced by `completion`, and the caret after it (a file is followed by a space). */
export function applyPath(text: string, p: PathAt, completion: string): { text: string; caret: number } {
  const after = text.slice(p.end);
  const file = p.files && !completion.endsWith("/");
  // A file ends the word: past the space after it (added when there is none).
  const ins = file && !/^\s/.test(after) ? `${completion} ` : completion;
  return { text: text.slice(0, p.start) + ins + after, caret: p.start + completion.length + (file ? 1 : 0) };
}
