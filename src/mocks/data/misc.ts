import type { AppInfo, Diagnostics, IndexStatus, SlashCommand, Stats } from "@/ipc/bindings";
import { DAY, NOW } from "./builders";

export const appInfo = {
  version: "0.1.0",
  dataRoot: "/Users/dev/.claude",
  dataRootSource: "default",
  dataRootExists: true,
  cacheDir: "/Users/dev/Library/Caches/dev.joy.codeck",
  schemaVersion: 1,
} satisfies AppInfo;

export const indexStatus = {
  phase: "idle",
  filesTotal: 1342,
  filesDone: 1342,
  bytesTotal: 3_221_225_472,
  bytesDone: 3_221_225_472,
  sessionsTotal: 10,
  textReady: true,
  error: null,
} satisfies IndexStatus;

export const diagnostics = {
  filesScanned: 1342,
  sessions: 10,
  emptySessions: 2,
  failedLines: 3,
  filesWithFailures: [
    {
      path: "/Users/dev/.claude/projects/-Users-dev-Developer-orbit-web/5e1c0b7a-0006-4000-8000-000000000006.jsonl",
      failedLines: 3,
      firstError: "EOF while parsing a string at line 412 column 18",
    },
  ],
  unknownEntryTypes: [{ name: "frame-link", count: 14, versions: ["2.1.88", "2.1.91"] }],
  unknownBlockTypes: [{ name: "server_tool_use", count: 5, versions: ["2.1.91"] }],
  unknownSystemSubtypes: [{ name: "memory_saved", count: 2, versions: ["2.1.91"] }],
  unknownTools: [{ name: "ScheduleWakeup", count: 9, versions: ["2.1.90", "2.1.91"] }],
  versions: [
    { version: "2.1.91", sessions: 6, failedLines: 0, unknownItems: 12, lastSeenMs: NOW - 120_000 },
    { version: "2.1.88", sessions: 3, failedLines: 3, unknownItems: 9, lastSeenMs: NOW - 5 * DAY },
    { version: "2.1.76", sessions: 1, failedLines: 0, unknownItems: 0, lastSeenMs: NOW - 20 * DAY },
  ],
  duplicateUuids: 2,
  orphanSubagents: 1,
} satisfies Diagnostics;

function localDay(ms: number): string {
  const d = new Date(ms);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

/** Deterministic pseudo-random in [0, 1). */
const rnd = (i: number) => {
  const x = Math.sin(i * 9301 + 49297) * 233280;
  return x - Math.floor(x);
};

const models = ["claude-sonnet-5-5", "claude-opus-4-7"];
const daily = Array.from({ length: 30 }, (_, i) => 29 - i).flatMap((back) =>
  models.map((model, m) => {
    const k = back * 2 + m;
    const scale = m === 0 ? 1 : 0.4;
    return {
      day: localDay(NOW - back * DAY),
      model,
      input: Math.round(rnd(k) * 40_000 * scale) + 2_000,
      output: Math.round(rnd(k + 100) * 180_000 * scale) + 5_000,
      cacheRead: Math.round(rnd(k + 200) * 4_000_000 * scale) + 100_000,
      cacheCreation: Math.round(rnd(k + 300) * 300_000 * scale) + 10_000,
    };
  }),
);

export const stats = {
  overview: {
    sessions: 10,
    messages: 1286,
    outputTokens: daily.reduce((s, d) => s + d.output, 0),
    activeDays: 24,
    cost: 412.37,
    unpricedMessages: 0,
  },
  daily,
  heatDaily: Array.from({ length: 90 }, (_, i) => ({
    day: localDay(NOW - (89 - i) * DAY),
    messages: Math.round(rnd(i + 7) * 80),
  })),
  // Filled per request by `getStats`.
  activity: { bucket: "day", bars: [] },
  // Filled from the mock sessions by `getStats`.
  sessions: [],
} satisfies Stats;

/** Completion while driving (`listSlashCommands`): a few built-in commands, a project command, a plugin skill. */
export const slashCommands: SlashCommand[] = [
  { name: "add-dir", description: "添加工作目录", source: "builtin", interactive: false },
  { name: "clear", description: "清空对话，开始新的 Session", source: "builtin", interactive: false },
  { name: "compact", description: "压缩对话上下文（可附压缩说明）", source: "builtin", interactive: false },
  { name: "cost", description: "查看用量与花费", source: "builtin", interactive: true },
  { name: "model", description: "切换模型（带模型名时直接切换）", source: "builtin", interactive: true },
  { name: "rename", description: "重命名 Session", source: "builtin", interactive: false },
  { name: "release-notes", description: "查看更新说明", source: "builtin", interactive: false },
  { name: "release", description: "按仓库的发布规则发版", source: "project", interactive: false },
  { name: "kit:review", description: "审查改动", source: "plugin", interactive: false },
  { name: "help", description: "帮助与命令列表", source: "builtin", interactive: true },
  { name: "vim", description: "切换 Vim 编辑模式", source: "builtin", interactive: false },
];

/** The paths `completePaths` knows (`/add-dir` and `@` completion), as on a disk: a directory ends in `/`. */
const mockPaths = [
  "/work/demo/apps/",
  "/work/demo/apps/kestrel/",
  "/work/demo/apps/kestrel/main.go",
  "/work/demo/clusters/",
  "/work/demo/.github/",
  "/work/demo/README.md",
  "/work/demo/Cargo.toml",
  "/work/infra/",
  "/work/infra-modules/",
  "/work/notes/",
  "/Users/dev/Downloads/",
];

/** `completePaths` over `mockPaths`, as the Engine does it (relative to `cwd`, `..` up, hidden ones after a `.`). */
export function completeMockPaths(cwd: string | null, partial: string, files: boolean): string[] {
  if (partial === "~") return ["~/"];
  const cut = partial.lastIndexOf("/") + 1;
  const [head, prefix] = [partial.slice(0, cut), partial.slice(cut)];
  const from = head.startsWith("/")
    ? head
    : head.startsWith("~/")
      ? `/Users/dev/${head.slice(2)}`
      : cwd && `${cwd}/${head}`;
  if (!from) return [];
  const parts: string[] = [];
  for (const p of from.split("/")) {
    if (p === "..") parts.pop();
    else if (p && p !== ".") parts.push(p);
  }
  const base = `/${parts.join("/")}`.replace(/\/$/, "") + "/";
  const names = mockPaths
    .filter((d) => d.startsWith(base) && d.length > base.length && !d.slice(base.length, -1).includes("/"))
    .map((d) => d.slice(base.length))
    .filter((n) => files || n.endsWith("/"))
    .filter((n) => (prefix.startsWith(".") || !n.startsWith(".")) && n.toLowerCase().startsWith(prefix.toLowerCase()))
    // By name, as the Engine sorts (without the directory's `/`).
    .sort((a, b) => a.replace(/\/$/, "").toLowerCase().localeCompare(b.replace(/\/$/, "").toLowerCase()));
  if (prefix === "." || prefix === "..") names.unshift("../");
  return names.map((n) => `${head}${n}`);
}
