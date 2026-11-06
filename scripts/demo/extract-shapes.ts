// Shape library for the demo data root: one real entry per kind, sanitized (examples/sanitize.rs), as templates
// for scripts/demo/build.ts. Reads a Claude Code data root read-only; writes demo/shapes/*.json.
//
//   node scripts/demo/extract-shapes.ts [--root ~/.claude] [--version 2.1.318]
//
// Run only to refresh the library (a new Claude Code version, a new kind). The output is committed.

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

type Entry = Record<string, any>;

const args = process.argv.slice(2);
const opt = (name: string, dflt: string) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1]! : dflt;
};
const ROOT = opt("--root", path.join(os.homedir(), ".claude")).replace(/^~/, os.homedir());
const VERSION = opt("--version", "2.1.318");
const REPO = path.resolve(import.meta.dirname, "../..");
const OUT = path.join(REPO, "demo/shapes");

/** Kinds the demo needs (main file); anything else found is ignored. */
const WANT = new Set([
  "user.prompt",
  "user.command",
  "user.local_stdout",
  "user.caveat",
  "user.bash_input",
  "user.bash_stdout",
  "user.image",
  "user.meta",
  "user.task_notification",
  "user.teammate",
  "user.peer",
  "user.compact_summary",
  "assistant.thinking",
  "assistant.text",
  "assistant.tool_use",
  "assistant.api_error",
  "system.compact_boundary",
  "system.turn_duration",
  "system.stop_hook_summary",
  "system.api_error",
  "system.informational",
  "system.local_command",
  "attachment.hook_success",
  "ai-title",
  "last-prompt",
  "permission-mode",
  "mode",
  "file-history-snapshot",
  ...[
    "Bash",
    "Bash.background",
    "Edit",
    "Write",
    "Read",
    "TodoWrite",
    "TaskCreate",
    "TaskUpdate",
    "Grep",
    "Glob",
    "WebFetch",
    "WebSearch",
    "mcp",
    "Agent.sync",
    "Agent.async",
    "Agent.teammate",
    "AskUserQuestion",
    "Workflow",
    "SendMessage",
  ].map((t) => `user.tool_result.${t}`),
]);

const text = (c: unknown): string =>
  typeof c === "string"
    ? c
    : Array.isArray(c)
      ? c.map((b) => (b && typeof b === "object" && b.type === "text" ? String(b.text ?? "") : "")).join(" ")
      : "";

function kindOf(d: Entry, tools: Map<string, string>): string {
  const t = d.type;
  if (t === "user") {
    const c = d.message?.content;
    if (d.isCompactSummary) return "user.compact_summary";
    if (Array.isArray(c) && c[0]?.type === "tool_result") {
      const name = tools.get(c[0].tool_use_id) ?? "?";
      const r = d.toolUseResult;
      if (name === "Bash" && r && typeof r === "object" && r.backgroundTaskId)
        return "user.tool_result.Bash.background";
      if (name === "Agent" || name === "Task") {
        if (r?.status === "teammate_spawned") return "user.tool_result.Agent.teammate";
        return r?.isAsync ? "user.tool_result.Agent.async" : "user.tool_result.Agent.sync";
      }
      return `user.tool_result.${name}`;
    }
    if (Array.isArray(c) && c.some((b) => b?.type === "image")) return "user.image";
    const s = text(c);
    for (const [tag, k] of [
      ["<command-name>", "command"],
      ["<local-command-stdout>", "local_stdout"],
      ["<local-command-caveat>", "caveat"],
      ["<bash-input>", "bash_input"],
      ["<bash-stdout>", "bash_stdout"],
      ["<task-notification>", "task_notification"],
      ["<teammate-message", "teammate"],
      ["<cross-session-message", "peer"],
    ] as const)
      if (s.includes(tag)) return `user.${k}`;
    if (d.isMeta) return "user.meta";
    return d.origin?.kind === "human" ? "user.prompt" : "user.other";
  }
  if (t === "assistant") {
    if (d.isApiErrorMessage) return "assistant.api_error";
    const b = d.message?.content?.[0];
    if (b?.type === "thinking" && !b.thinking) return "assistant.thinking_empty";
    return `assistant.${b?.type ?? "empty"}`;
  }
  if (t === "system") return `system.${d.subtype}`;
  if (t === "attachment") return `attachment.${d.attachment?.type}`;
  return String(t);
}

const files = fs
  .readdirSync(path.join(ROOT, "projects"), { withFileTypes: true })
  .filter((e) => e.isDirectory())
  .flatMap((e) => {
    const dir = path.join(ROOT, "projects", e.name);
    return fs
      .readdirSync(dir)
      .filter((f) => f.endsWith(".jsonl"))
      .map((f) => path.join(dir, f));
  })
  .map((f) => ({ f, m: fs.statSync(f).mtimeMs }))
  .sort((a, b) => b.m - a.m)
  .map((x) => x.f);

const picked = new Map<string, string>();
/** Kinds absent from VERSION: the most recent sample of another version. */
const fallback = new Map<string, string>();
for (const f of files) {
  if (picked.size === WANT.size) break;
  const tools = new Map<string, string>();
  for (const line of fs.readFileSync(f, "utf8").split("\n")) {
    if (!line) continue;
    let d: Entry;
    try {
      d = JSON.parse(line);
    } catch {
      continue;
    }
    if (d.type === "assistant")
      for (const b of d.message?.content ?? [])
        if (b?.type === "tool_use") tools.set(b.id, String(b.name).startsWith("mcp__") ? "mcp" : b.name);
    const k = kindOf(d, tools);
    if (!WANT.has(k) || picked.has(k)) continue;
    if (line.length > 40_000) continue;
    if (d.version && d.version !== VERSION) {
      if (!fallback.has(k)) fallback.set(k, line);
      continue;
    }
    picked.set(k, line);
  }
}
const versions = new Map<string, string>();
for (const k of WANT) {
  if (picked.has(k) || !fallback.has(k)) continue;
  picked.set(k, fallback.get(k)!);
  versions.set(k, JSON.parse(fallback.get(k)!).version ?? "?");
}

const kinds = [...picked.keys()].sort();
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "demo-shapes-"));
const raw = path.join(tmp, "raw.jsonl");
fs.writeFileSync(raw, kinds.map((k) => picked.get(k)).join("\n") + "\n");
const out = execFileSync(
  "cargo",
  ["run", "-q", "-p", "codeck-core", "--example", "sanitize", "--", raw, "--cwd", "/Users/dev/code/lumen-api"],
  { cwd: REPO, encoding: "utf8", maxBuffer: 64 << 20 },
)
  .split("\n")
  .filter(Boolean);
fs.rmSync(tmp, { recursive: true });
if (out.length !== kinds.length) throw new Error(`sanitize returned ${out.length} lines for ${kinds.length} kinds`);

// The builder sets every id; the sampled ones (the machine's own sessions, messages, requests) are not kept.
let n = 0;
const ids = new Map<string, string>();
const scrub = (line: string) =>
  line.replace(
    /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b|\b(?:toolu|msg|req|srvtoolu)_[0-9A-Za-z]{10,}\b/g,
    (id) => {
      if (!ids.has(id)) {
        const k = String(++n).padStart(12, "0");
        const prefix = /^[a-z]+_/.exec(id)?.[0];
        ids.set(id, prefix ? `${prefix}01${"0".repeat(10)}${k}` : `00000000-0000-4000-8000-${k}`);
      }
      return ids.get(id)!;
    },
  );

fs.mkdirSync(OUT, { recursive: true });
kinds.forEach((k, i) =>
  fs.writeFileSync(path.join(OUT, `${k}.json`), JSON.stringify(JSON.parse(scrub(out[i]!)), null, 1) + "\n"),
);
const missing = [...WANT].filter((k) => !picked.has(k));
console.log(`${kinds.length} shapes written to demo/shapes (claude ${VERSION})`);
for (const [k, v] of versions) console.log(`  ${k}: from ${v}`);
if (missing.length) console.log(`not found: ${missing.join(", ")}`);
