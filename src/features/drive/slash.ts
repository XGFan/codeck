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

/** The command named at the start of `body` (`/name args`), when it is one in `list`. */
export function commandOf(body: string, list: SlashCommand[] | undefined): SlashCommand | null {
  if (!body.startsWith("/")) return null;
  const name = body.slice(1).split(/\s/, 1)[0];
  return (name && list?.find((c) => c.name === name)) || null;
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
