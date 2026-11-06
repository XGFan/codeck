import type { LiveState, Node, SessionSummary, TitleSource, TokenTotals } from "@/ipc/bindings";
import { asstNode, call, costOf, DAY, HOUR, MIN, NOW, sumTok, tok, userNode } from "./builders";
import { HOMELAB, LUMEN, ORBIT } from "./projects";
import { RICH_ID, richMain, richSubagents } from "./rich";

const sid = (n: number) =>
  `${["7f3a9c10", "2b8d41e5", "c04e77a2", "91d5be38", "e6a2f0c4", "5e1c0b7a", "3d9f84b1", "a7c63e92", "48b1d0f6", "d2e95a07"][n - 1]}-${String(n).padStart(4, "0")}-4000-8000-${String(n).padStart(12, "0")}`;

export const IDS = {
  rich: RICH_ID,
  forkOrigin: sid(2),
  forkChild: sid(3),
  redis: sid(4),
  orbitLive: sid(5),
  orbitZod: sid(6),
  orbitReview: sid(7),
  homelabFlux: sid(8),
  homelabLonghorn: sid(9),
  untitled: sid(10),
};

interface Turn {
  u: string;
  /** Output of a local slash command, shown right after it. */
  out?: string;
  /** Omitted for a local command, which gets no reply. */
  a?: string;
  /** A Bash call attached to the assistant reply. */
  bash?: { command: string; out: string; isError?: boolean };
}

interface Def {
  id: string;
  projectId: string;
  /** Projects an edit tool wrote to (`projectId` included if edited). */
  editedProjectIds?: string[];
  title: string;
  titleSource: TitleSource;
  createdAgo: number;
  lastActiveAgo: number;
  gitBranch: string | null;
  live: LiveState | null;
  model: string | null;
  tokens: TokenTotals;
  subagentCount?: number;
  turns: Turn[];
  forkOf?: { id: string; forkTurns: number };
}

const live = (status: LiveState["status"], pid: number | null, waitingFor: string | null = null): LiveState => ({
  status,
  rawStatus: status === "unknown" ? null : status,
  waitingFor,
  pid,
});

const authTurns: Turn[] = [
  {
    u: "把 auth 模块里 token 校验和会话存储拆开，先给出拆分方案。",
    a: "建议拆成 `TokenVerifier`（纯函数，校验签名与过期）与 `SessionStore`（trait，Redis 实现）两部分。",
  },
  {
    u: "按这个方案改，先抽 TokenVerifier。",
    a: "已抽出 `TokenVerifier`，原 `AuthService::verify` 改为委托调用。",
    bash: { command: "cargo test -p lumen-auth", out: "test result: ok. 37 passed; 0 failed" },
  },
  {
    u: "SessionStore 的 Redis 实现里连接是怎么复用的？",
    a: "通过 `deadpool-redis` 连接池复用，池大小默认 16，见 `config.redis.pool_size`。",
  },
  { u: "好，继续把 SessionStore 抽成 trait。", a: "已抽成 `trait SessionStore`，并提供 `InMemoryStore` 供测试使用。" },
];

const defs: Def[] = [
  {
    id: IDS.rich,
    projectId: LUMEN,
    title: "为 /v1/chat 接口增加令牌桶限流中间件",
    titleSource: "ai",
    createdAgo: 3 * HOUR,
    lastActiveAgo: 2 * MIN,
    gitBranch: "feat/rate-limit",
    live: live("busy", 48211),
    model: "claude-sonnet-5-5",
    tokens: tok(0, 0),
    turns: [],
  },
  {
    id: IDS.forkOrigin,
    projectId: LUMEN,
    title: "重构认证模块：拆分 token 校验与会话存储",
    titleSource: "custom",
    createdAgo: 2 * DAY,
    lastActiveAgo: 2 * DAY - 40 * MIN,
    gitBranch: "refactor/auth",
    live: null,
    model: "claude-opus-4-7",
    tokens: tok(31_000, 24_800, 1_200_000, 64_000),
    turns: authTurns,
  },
  {
    id: IDS.forkChild,
    projectId: LUMEN,
    title: "认证模块重构（改用 JWT 轮换）",
    titleSource: "ai",
    createdAgo: DAY + 3 * HOUR,
    lastActiveAgo: DAY,
    gitBranch: "refactor/auth-jwt",
    live: null,
    model: "claude-opus-4-7",
    tokens: tok(18_000, 15_200, 800_000, 31_000),
    turns: [
      {
        u: "改成 JWT 轮换：access 15 分钟，refresh 7 天。",
        a: "已在 `TokenVerifier` 中加入 `kid` 轮换，refresh token 存入 `SessionStore` 并做一次性使用。",
      },
      {
        u: "补一个轮换的单测。",
        a: "已新增 `rotation_rejects_reused_refresh_token`。",
        bash: { command: "cargo test rotation", out: "test result: ok. 3 passed; 0 failed" },
      },
    ],
    forkOf: { id: IDS.forkOrigin, forkTurns: 3 },
  },
  {
    id: IDS.redis,
    projectId: LUMEN,
    title: "排查 Redis 连接池泄漏",
    titleSource: "ai",
    createdAgo: 4 * DAY,
    lastActiveAgo: 4 * DAY - HOUR,
    gitBranch: "fix/redis-leak",
    live: null,
    model: "claude-sonnet-5-5",
    tokens: tok(12_400, 9_300, 640_000, 20_000),
    turns: [
      {
        u: "线上 Redis 连接数持续增长，帮我排查连接池泄漏。",
        a: "先确认是否有未归还的连接：检查 `deadpool` 的 `status()` 指标与 handler 中的 `?` 提前返回。",
        bash: { command: 'rg -n "pool.get\\(\\)" src', out: "src/session.rs:44:    let mut conn = pool.get().await?;" },
      },
      {
        u: "session.rs 第 44 行看起来没问题？",
        a: "问题在 `spawn` 出去的任务持有 `conn` 跨越了 `.await` 且未设超时，超时任务被丢弃前连接不会归还。",
      },
    ],
  },
  {
    id: IDS.orbitLive,
    projectId: ORBIT,
    title: "优化首页首屏加载（LCP 3.2s → 1.8s）",
    titleSource: "ai",
    createdAgo: 5 * HOUR,
    lastActiveAgo: 8 * MIN,
    gitBranch: "perf/lcp",
    live: live("waiting", null, "permission prompt"),
    model: "claude-sonnet-5-5",
    tokens: tok(22_000, 18_600, 1_900_000, 45_000),
    turns: [
      {
        u: "首页 LCP 3.2s，帮我定位瓶颈并优化到 2s 以内。",
        a: "LCP 元素是首屏轮播图，未设置 `fetchpriority`，且被 3 个同步脚本阻塞。",
        bash: { command: "pnpm lighthouse --only-categories=performance", out: "LCP: 3.2 s\nTBT: 410 ms" },
      },
      {
        u: "先把轮播首图预加载，再把脚本改成 defer。",
        a: '已为首图添加 `<link rel="preload" as="image" fetchpriority="high">`，三处脚本改为 `defer`。LCP 降至 1.8 s。',
        bash: { command: "pnpm lighthouse --only-categories=performance", out: "LCP: 1.8 s\nTBT: 120 ms" },
      },
    ],
  },
  {
    id: IDS.orbitZod,
    projectId: ORBIT,
    editedProjectIds: [ORBIT, LUMEN],
    title: "迁移表单校验到 zod",
    titleSource: "ai",
    createdAgo: 3 * DAY,
    lastActiveAgo: 3 * DAY - 2 * HOUR,
    gitBranch: "chore/zod",
    live: null,
    model: "claude-sonnet-5-5",
    tokens: tok(15_800, 13_100, 1_100_000, 38_000),
    turns: [
      {
        u: "把注册表单的校验从 yup 迁移到 zod，并保持错误文案不变。",
        a: '已迁移 `signupSchema`，错误文案通过 `z.string().min(8, "密码至少 8 位")` 保持一致。',
      },
      { u: "把登录表单也迁了。", a: "登录表单已迁移，`yup` 依赖仅剩 2 处引用。" },
    ],
  },
  {
    id: IDS.orbitReview,
    projectId: ORBIT,
    title: "/review 订单列表页",
    titleSource: "firstPrompt",
    createdAgo: 8 * DAY,
    lastActiveAgo: 8 * DAY - 30 * MIN,
    gitBranch: "feat/orders",
    live: null,
    model: "claude-opus-4-7",
    tokens: tok(8_900, 5_400, 420_000, 12_000),
    turns: [
      { u: "/review 订单列表页", a: "发现 2 个问题：分页参数未做上限校验；列表在筛选变化时未取消上一个请求。" },
      { u: "!git status", out: "<bash-stdout>On branch feat/orders</bash-stdout><bash-stderr></bash-stderr>" },
      {
        u: "/rename 订单列表页审查",
        out: "<local-command-stdout>Session renamed to: \u001b[1m订单列表页审查\u001b[22m</local-command-stdout>",
      },
    ],
    subagentCount: 1,
  },
  {
    id: IDS.homelabFlux,
    projectId: HOMELAB,
    title: "为 Flux 增加 kestrel-cli 镜像自动更新",
    titleSource: "ai",
    createdAgo: 6 * DAY,
    lastActiveAgo: 6 * DAY - 3 * HOUR,
    gitBranch: "main",
    live: null,
    model: "claude-sonnet-5-5",
    tokens: tok(10_200, 8_700, 560_000, 15_000),
    turns: [
      {
        u: "给 kestrel-cli 的镜像加上 Flux ImageUpdateAutomation，只跟踪 semver 1.x。",
        a: "已添加 `ImageRepository`、`ImagePolicy`（semver `1.x`）与 `ImageUpdateAutomation`。",
      },
    ],
  },
  {
    id: IDS.homelabLonghorn,
    projectId: HOMELAB,
    title: "Longhorn 卷扩容与备份策略",
    titleSource: "custom",
    createdAgo: 12 * DAY,
    lastActiveAgo: 12 * DAY - 2 * HOUR,
    gitBranch: "main",
    live: null,
    model: "claude-sonnet-5-5",
    tokens: tok(9_400, 7_100, 480_000, 11_000),
    turns: [
      {
        u: "把 postgres 的 PVC 从 20Gi 扩到 50Gi，并设置每日备份保留 7 份。",
        a: "已修改 PVC 请求为 50Gi，并新增 `RecurringJob`（每日 03:00，retain=7）。",
      },
    ],
  },
  {
    id: IDS.untitled,
    projectId: ORBIT,
    title: "（无标题）",
    titleSource: "untitled",
    createdAgo: 20 * DAY,
    lastActiveAgo: 20 * DAY - 10 * MIN,
    gitBranch: null,
    live: null,
    model: null,
    tokens: tok(300, 120, 0, 0),
    turns: [],
  },
];

/** Main + Subagent cost of the rich session (each run at its own model's rate). */
export function richModelCosts(): { model: string; cost: number; unpricedMessages: number }[] {
  const byModel = new Map<string, number>([
    ["claude-sonnet-5-5", costOf("claude-sonnet-5-5", tok(61_000, 42_400, 1_450_000, 88_000))!],
  ]);
  for (const a of richSubagents) byModel.set(a.model, (byModel.get(a.model) ?? 0) + (costOf(a.model, a.tokens) ?? 0));
  return [...byModel].map(([model, cost]) => ({ model, cost, unpricedMessages: 0 })).sort((a, b) => b.cost - a.cost);
}

function richCost() {
  return { cost: richModelCosts().reduce((s, m) => s + m.cost, 0), unpricedMessages: 0 };
}

function turnNodes(prefix: string, startMs: number, turns: Turn[], inheritedCount = 0): Node[] {
  const nodes: Node[] = [];
  turns.forEach((turn, i) => {
    const t0 = startMs + i * 3 * MIN;
    const uInherited = nodes.length < inheritedCount;
    const cmd = turn.u.match(/^(\/\S+|!)\s*(.*)$/s);
    const origin = cmd ? ({ kind: "command", name: cmd[1]!, args: cmd[2]! } as const) : undefined;
    nodes.push(userNode(`${prefix}-u${i}`, t0, turn.u, { inherited: uInherited, origin }));
    if (turn.out) nodes.push(userNode(`${prefix}-o${i}`, t0, turn.out, { origin: { kind: "commandOutput" } }));
    if (turn.a === undefined) return;
    const blocks = [{ kind: "text", text: turn.a } as const];
    const all = turn.bash
      ? [
          ...blocks,
          call(
            `${prefix}-tu${i}`,
            "Bash",
            { command: turn.bash.command },
            { text: turn.bash.out, isError: turn.bash.isError ?? false },
          ),
        ]
      : blocks;
    nodes.push(asstNode(`${prefix}-a${i}`, t0 + MIN, all, { inherited: nodes.length < inheritedCount }));
  });
  return nodes;
}

export interface MockSessionData {
  summary: SessionSummary;
  /** Main path nodes; Rich session overrides this with `richMain`. */
  nodes: Node[];
  forkOf?: { id: string; count: number; lastInheritedId: string };
}

const originDef = defs.find((d) => d.id === IDS.forkOrigin)!;

export const sessionData: MockSessionData[] = defs.map((d) => {
  const created = NOW - d.createdAgo;
  let nodes: Node[];
  let forkOf: MockSessionData["forkOf"];
  if (d.id === IDS.rich) {
    nodes = richMain;
  } else if (d.forkOf) {
    const inheritedNodes = turnNodes("fo", NOW - originDef.createdAgo, originDef.turns).slice(
      0,
      d.forkOf.forkTurns * 2,
    );
    inheritedNodes.forEach((n) => (n.inherited = true));
    nodes = [...inheritedNodes, ...turnNodes("fc", created + 10 * MIN, d.turns)];
    forkOf = {
      id: d.forkOf.id,
      count: inheritedNodes.length,
      lastInheritedId: inheritedNodes[inheritedNodes.length - 1]!.id,
    };
  } else {
    nodes = turnNodes(d.id.slice(0, 4), created, d.turns);
  }

  const richTokens = sumTok(tok(61_000, 42_400, 1_450_000, 88_000), ...richSubagents.map((s) => s.tokens));
  const messageCount =
    d.id === IDS.rich
      ? 48
      : nodes.filter((n) => !(n.body.kind === "userPrompt" && n.body.origin.kind === "commandOutput")).length;
  const summary: SessionSummary = {
    id: d.id,
    projectId: d.projectId,
    title: d.title,
    titleSource: d.titleSource,
    createdMs: created,
    lastActiveMs: NOW - d.lastActiveAgo,
    lastPromptMs: NOW - d.lastActiveAgo,
    messageCount,
    toolCallCount:
      d.id === IDS.rich
        ? 27
        : nodes.filter((n) => n.body.kind === "assistant" && n.body.blocks.some((b) => b.kind === "toolCall")).length,
    subagentCount: d.id === IDS.rich ? richSubagents.length : (d.subagentCount ?? 0),
    tokens: d.id === IDS.rich ? richTokens : d.tokens,
    ...(d.id === IDS.rich
      ? richCost()
      : { cost: costOf(d.model, d.tokens) ?? 0, unpricedMessages: costOf(d.model, d.tokens) == null ? 1 : 0 }),
    gitBranch: d.gitBranch,
    live: d.live,
    forkOrigin: d.forkOf
      ? { sessionId: d.forkOf.id, title: originDef.title, forkPointId: forkOf!.lastInheritedId }
      : null,
    primaryModel: d.model,
    editedProjectIds: d.editedProjectIds ?? [],
  };
  return { summary, nodes, forkOf };
});
