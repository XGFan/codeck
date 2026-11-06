// The demo transcript generator's core: entries are deep copies of real, sanitized entries (demo/shapes/*.json)
// with ids, links, times and text filled in the way Claude Code writes them. Stories (demo/story/*.ts) drive it.

import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";

export type Entry = Record<string, any>;

const SHAPES = path.resolve(import.meta.dirname, "../../demo/shapes");
const cache = new Map<string, Entry>();
/** A fresh copy of the real entry of `kind`. */
export function shape(kind: string): Entry {
  if (!cache.has(kind)) cache.set(kind, JSON.parse(fs.readFileSync(path.join(SHAPES, `${kind}.json`), "utf8")));
  return structuredClone(cache.get(kind)!);
}

/** Envelope fields every chained entry carries; filled per session. */
const ENVELOPE = ["userType", "entrypoint", "cwd", "sessionId", "version", "gitBranch"] as const;
/** Fields of the sampled assistant entries that came from a plugin of the machine they were recorded on. */
const LOCAL_ONLY = ["attributionSkill", "attributionPlugin", "promptCacheRequested"];

const B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/** Deterministic ids: the same story gives byte-identical output. */
export class Ids {
  private n = 0;
  private seed: string;
  constructor(seed: string) {
    this.seed = seed;
  }
  private next(tag: string) {
    return createHash("sha256").update(`${this.seed}/${tag}/${this.n++}`).digest();
  }
  uuid() {
    const h = this.next("uuid").toString("hex");
    return `${h.slice(0, 8)}-${h.slice(8, 12)}-4${h.slice(13, 16)}-${"89ab"[parseInt(h[16]!, 16) % 4]}${h.slice(17, 20)}-${h.slice(20, 32)}`;
  }
  private b58(tag: string, len: number) {
    const h = this.next(tag);
    let s = "";
    for (let i = 0; i < len; i++) s += B58[h[i % h.length]! % 58];
    return s;
  }
  msg = () => `msg_011C${this.b58("msg", 20)}`;
  req = () => `req_011C${this.b58("req", 20)}`;
  tool = () => `toolu_01${this.b58("tool", 22)}`;
  /** Background task ids: an agent's (`a` + 16 hex) or a shell's (9 alphanumerics). */
  agent = () => `a${this.next("agent").toString("hex").slice(0, 16)}`;
  shell = () => this.b58("shell", 9).toLowerCase();
}

export interface SessionOpts {
  id: string;
  /** Project directory name under /Users/dev/code. */
  project: string;
  branch: string;
  /** ISO start time. */
  start: string;
  version?: string;
  model?: string;
  title: string;
  permissionMode?: string;
  slug?: string;
}

export interface ToolOut {
  /** tool_result content: a string or content blocks. */
  content: string | Entry[];
  toolUseResult: unknown;
  isError?: boolean;
  /** Time the call took. */
  ms?: number;
  /** Extra top-level fields of the result entry (e.g. `sandboxed`). */
  extra?: Entry;
}

export interface Call {
  name: string;
  input: Entry;
  out: ToolOut;
}

/** One file of entries (the main transcript or a Subagent's) sharing a session's ids and clock. */
export class Writer {
  lines: Entry[] = [];
  /** Uuid of the chain's last entry (the next entry's parent). */
  last: string | null = null;
  promptId = "";
  promptIndex = 0;
  turnIndex = 0;
  /** Running context size, for plausible usage numbers. */
  ctx = 18_000;
  private open: { id: string; req: string; ref: string; index: number; usage: Entry } | null = null;

  /** The main file uses the session's clock; a Subagent runs alongside it on its own. */
  readonly clock: { now: number };
  readonly s: Session;
  readonly agentId: string | null;
  constructor(s: Session, agentId: string | null = null, start?: number) {
    this.s = s;
    this.agentId = agentId;
    this.clock = start == null ? s.clock : { now: start };
  }

  get now() {
    return this.clock.now;
  }
  tick(ms: number) {
    this.clock.now += ms;
  }

  /** Stamps envelope, uuid, parent and time on `e` and appends it to the chain. */
  push(e: Entry, opts: { parent?: string | null; chain?: boolean } = {}): Entry {
    for (const k of LOCAL_ONLY) delete e[k];
    e.parentUuid = opts.parent !== undefined ? opts.parent : this.last;
    e.isSidechain = this.agentId != null;
    if (this.agentId) e.agentId = this.agentId;
    e.uuid = this.s.ids.uuid();
    e.timestamp = new Date(this.now).toISOString();
    for (const k of ENVELOPE) e[k] = this.s.env[k];
    // Attachments and meta prompts carry it as well, even where the sampled entry did not.
    if ("session_id" in e || e.type === "attachment" || (e.type === "user" && e.isMeta))
      e.session_id = this.s.env.sessionId;
    if ("slug" in e) e.slug = this.s.slug;
    this.lines.push(order(e));
    if (opts.chain !== false) this.last = e.uuid;
    return e;
  }

  /** A meta line without a place in the chain (title, mode, snapshot…). */
  meta(e: Entry) {
    if ("sessionId" in e || e.type !== "file-history-snapshot") e.sessionId = this.s.env.sessionId;
    this.lines.push(e);
  }

  private closeMessage() {
    this.open = null;
  }

  /** A content block of the assistant message being written (a new message when none is open). */
  private block(kind: "thinking" | "text" | "tool_use", block: Entry, stop: string | null, extra: Entry = {}) {
    if (!this.open) {
      const out = 80 + JSON.stringify(block).length / 3;
      this.ctx += 900;
      this.open = {
        id: this.s.ids.msg(),
        req: this.s.ids.req(),
        ref: this.s.ids.uuid(),
        index: 0,
        usage: usage(this.ctx, out),
      };
    }
    const e = shape(`assistant.${kind}`);
    e.message.model = this.s.model;
    e.message.id = this.open.id;
    e.message.content = [block];
    e.message.stop_reason = stop;
    e.message.usage = this.open.usage;
    e.requestId = this.open.req;
    e.requestRef = this.open.ref;
    e.requestedModel = this.s.model;
    e.advisorModel = this.s.model;
    e.apiBlockIndex = this.open.index++;
    if (kind !== "tool_use") delete e.wireToolInputs;
    Object.assign(e, extra);
    return this.push(e);
  }

  think(text: string, ms = 3000) {
    this.tick(ms);
    return this.block("thinking", { type: "thinking", thinking: text }, null, {
      thinkingDurationMs: ms,
      thinkingDisplay: "updates",
    });
  }

  /** Text Claude writes; `end` closes the turn's message (`end_turn`). */
  say(text: string, opts: { end?: boolean } = {}) {
    this.tick(1500 + text.length * 25);
    const e = this.block("text", { type: "text", text }, opts.end ? "end_turn" : null);
    if (opts.end) this.closeMessage();
    return e;
  }

  /** Tool calls of one message (parallel when several) and their results; returns the result entries. */
  calls(list: Call[]): Entry[] {
    this.tick(1200);
    const uses = list.map((c) => {
      const id = this.s.ids.tool();
      const block = { type: "tool_use", id, name: c.name, input: c.input, caller: { type: "direct" } };
      const mcp = /^mcp__([^_]+(?:_[^_]+)*?)__(.+)$/.exec(c.name);
      const extra: Entry = { wireToolInputs: { [id]: c.input } };
      if (mcp) Object.assign(extra, { attributionMcpServer: mcp[1], attributionMcpTool: mcp[2] });
      const e = this.block("tool_use", block, "tool_use", extra);
      return { c, e, id };
    });
    this.closeMessage();
    const results = uses.map(({ c, e, id }) => {
      this.tick(c.out.ms ?? 800);
      const r = shape(resultKind(c));
      r.message = {
        role: "user",
        content: [
          {
            tool_use_id: id,
            type: "tool_result",
            content: c.out.content,
            ...(c.out.isError ? { is_error: true } : {}),
          },
        ],
      };
      r.promptId = this.promptId;
      r.sourceToolAssistantUUID = e.uuid;
      r.toolUseResult = c.out.toolUseResult;
      if ("permissionDecision" in r)
        r.permissionDecision = { decision: "accept", source: "config", reasonType: "mode" };
      delete r.sessionKind;
      Object.assign(r, c.out.extra ?? {});
      // Each result hangs off its own tool_use fragment.
      return this.push(r, { parent: e.uuid, chain: false });
    });
    this.last = results[results.length - 1]!.uuid;
    for (const { c } of uses) this.ctx += JSON.stringify(c.out.content).length / 3;
    return results.map((r, i) => Object.assign(r, { _toolUseId: uses[i]!.id }));
  }

  call(name: string, input: Entry, out: ToolOut): Entry {
    return this.calls([{ name, input, out }])[0]!;
  }

  /** Bookkeeping after a turn (hidden by default in CoDeck); `pending`: background agents still running. */
  endTurn(startedAt: number, messages: number, pending = 0) {
    this.closeMessage();
    const st = shape("system.stop_hook_summary");
    Object.assign(st, {
      hasOutput: false,
      hookAdditionalContext: [],
      hookCount: 1,
      hookErrors: [],
      hookInfos: [{ command: "~/.claude/hooks/notify.sh" }],
      preventedContinuation: false,
      stopReason: "",
      toolUseID: this.s.ids.uuid(),
      level: "suggestion",
    });
    this.push(st);
    const td = shape("system.turn_duration");
    Object.assign(td, { durationMs: this.now - startedAt, messageCount: messages });
    if (pending > 0) td.pendingBackgroundAgentCount = pending;
    else delete td.pendingBackgroundAgentCount;
    this.push(td);
  }
}

function resultKind(c: Call): string {
  const r = c.out.toolUseResult as Entry | undefined;
  if (c.name === "Bash" && r && typeof r === "object" && r.backgroundTaskId) return "user.tool_result.Bash.background";
  if (c.name === "Agent")
    return r?.status === "teammate_spawned" ? "user.tool_result.Agent.teammate" : "user.tool_result.Agent.async";
  if (c.name.startsWith("mcp__")) return "user.tool_result.mcp";
  return `user.tool_result.${c.name}`;
}

function usage(ctx: number, out: number): Entry {
  const read = Math.max(0, Math.round(ctx - 1200));
  const created = 1200;
  return {
    input_tokens: 2,
    cache_creation_input_tokens: created,
    cache_read_input_tokens: read,
    output_tokens: Math.max(20, Math.round(out)),
    server_tool_use: { web_search_requests: 0, web_fetch_requests: 0 },
    service_tier: "standard",
    cache_creation: { ephemeral_1h_input_tokens: created, ephemeral_5m_input_tokens: 0 },
    inference_geo: "global",
    speed: "standard",
  };
}

/** Key order as Claude Code writes it: links first, then type and payload, envelope last. */
function order(e: Entry): Entry {
  const head = ["parentUuid", "logicalParentUuid", "isSidechain", "agentId", "promptId", "type"];
  const tail = [...ENVELOPE];
  const out: Entry = {};
  for (const k of head) if (k in e) out[k] = e[k];
  for (const k of Object.keys(e)) if (!head.includes(k) && !tail.includes(k as never)) out[k] = e[k];
  for (const k of tail) if (k in e) out[k] = e[k];
  return out;
}

export class Session {
  ids: Ids;
  clock: { now: number };
  /** The session's slug (Claude Code names each session with three words). */
  slug: string;
  /** Text of the last prompt typed, for the `last-prompt` line. */
  lastPrompt = "";
  /** Named points of the chain (uuids) other stories refer to, e.g. where a Fork starts. */
  marks = new Map<string, string>();
  env: Record<(typeof ENVELOPE)[number], string>;
  model: string;
  main: Writer;
  /** Sidecar files: path relative to the session directory → lines or JSON. */
  sidecars = new Map<string, string>();
  readonly o: SessionOpts;

  constructor(o: SessionOpts) {
    this.o = o;
    this.ids = new Ids(o.id);
    this.clock = { now: Date.parse(o.start) };
    this.model = o.model ?? "claude-opus-5-5";
    this.slug = o.slug ?? "steady-token-bucket";
    this.env = {
      userType: "external",
      entrypoint: "cli",
      cwd: `/Users/dev/code/${o.project}`,
      sessionId: o.id,
      version: o.version ?? "2.1.318",
      gitBranch: o.branch,
    };
    this.main = new Writer(this);
    const pm = shape("permission-mode");
    pm.permissionMode = o.permissionMode ?? "default";
    this.main.meta(pm);
  }

  get dir() {
    return `-Users-dev-code-${this.o.project}`;
  }

  /** Writes the main file and sidecars under `<root>/projects/<dir>/`. */
  write(root: string) {
    const w = this.main;
    const t = shape("ai-title");
    t.aiTitle = this.o.title;
    w.meta(t);
    const lp = shape("last-prompt");
    lp.lastPrompt = this.lastPrompt;
    lp.leafUuid = w.last;
    w.meta(lp);
    const dir = path.join(root, "projects", this.dir);
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(path.join(dir, `${this.o.id}.jsonl`), jsonl(w.lines));
    for (const [rel, body] of this.sidecars) {
      const p = path.join(dir, this.o.id, rel);
      fs.mkdirSync(path.dirname(p), { recursive: true });
      fs.writeFileSync(p, body);
    }
  }
}

/**
 * A Session forked from `origin` at its entry `atUuid`: the child repeats every uuid-bearing entry up to that one
 * (same uuids, `forkedFrom` pointing at itself in the origin, the origin's `session_id` kept), then goes on alone.
 */
export function forkSession(o: SessionOpts, origin: Session, atUuid: string): Session {
  const child = new Session(o);
  const w = child.main;
  for (const e of origin.main.lines) {
    if (!e.uuid) continue;
    const c = structuredClone(e);
    c.sessionId = o.id;
    c.forkedFrom = { sessionId: origin.o.id, messageUuid: e.uuid };
    w.lines.push(c);
    if (e.uuid === atUuid) break;
  }
  w.last = atUuid;
  child.clock.now = Date.parse(o.start);
  return child;
}

export const jsonl = (lines: Entry[]) => lines.map((l) => JSON.stringify(l)).join("\n") + "\n";

/* ───────────── PNG (for pasted screenshots) ───────────── */

/** A small RGB PNG drawn from rectangles: `[x, y, w, h, [r, g, b]]`. */
export function png(
  width: number,
  height: number,
  bg: number[],
  rects: Array<[number, number, number, number, number[]]>,
) {
  const px = Buffer.alloc((width * 3 + 1) * height);
  for (let y = 0; y < height; y++) {
    px[y * (width * 3 + 1)] = 0;
    for (let x = 0; x < width; x++) {
      let c = bg;
      for (const [rx, ry, rw, rh, rc] of rects) if (x >= rx && x < rx + rw && y >= ry && y < ry + rh) c = rc;
      const o = y * (width * 3 + 1) + 1 + x * 3;
      px[o] = c[0]!;
      px[o + 1] = c[1]!;
      px[o + 2] = c[2]!;
    }
  }
  const crcTable = Array.from({ length: 256 }, (_, n) => {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    return c >>> 0;
  });
  const crc = (b: Buffer) => {
    let c = 0xffffffff;
    for (const x of b) c = crcTable[(c ^ x) & 0xff]! ^ (c >>> 8);
    return (c ^ 0xffffffff) >>> 0;
  };
  const chunk = (type: string, data: Buffer) => {
    const len = Buffer.alloc(4);
    len.writeUInt32BE(data.length);
    const td = Buffer.concat([Buffer.from(type), data]);
    const c = Buffer.alloc(4);
    c.writeUInt32BE(crc(td));
    return Buffer.concat([len, td, c]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8;
  ihdr[9] = 2;
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", zlib.deflateSync(px)),
    chunk("IEND", Buffer.alloc(0)),
  ]).toString("base64");
}
