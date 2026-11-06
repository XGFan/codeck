// Background Subagents: the Agent call in the main file, the Subagent's own transcript in
// `<session>/subagents/agent-<id>.jsonl` (+ `.meta.json`), and the task notification when it ends.

import { agentLaunched, agentNotification, taskNotification } from "./kit.ts";
import { type Entry, Writer, jsonl, shape } from "./lib.ts";

export interface AgentSpec {
  type: string;
  description: string;
  prompt: string;
  model?: "sonnet" | "opus" | "haiku";
  /** The Subagent's work; returns its final answer (written as its last message). */
  run: (sub: Writer) => string;
  /** Failed runs end with an error instead of an answer. */
  failure?: string;
}

export interface Launched {
  agentId: string;
  toolUseId: string;
  spec: AgentSpec;
  /** When the Subagent's last entry was written. */
  endedAt: number;
  result: string;
}

const MODEL: Record<string, string> = {
  sonnet: "claude-sonnet-5-5",
  opus: "claude-opus-5-5",
  haiku: "claude-haiku-5-5",
};

/** Launches Subagents from one assistant message (parallel when several) and writes their sidecars. */
export function launch(w: Writer, specs: AgentSpec[]): Launched[] {
  const s = w.s;
  const ids = specs.map(() => s.ids.agent());
  const results = w.calls(
    specs.map((spec, i) => ({
      name: "Agent",
      input: {
        description: spec.description,
        subagent_type: spec.type,
        model: spec.model ?? "sonnet",
        prompt: spec.prompt,
      },
      out: agentLaunched(s, ids[i]!, spec.description, spec.prompt),
    })),
  );
  return specs.map((spec, i) => {
    const agentId = ids[i]!;
    const toolUseId = results[i]!._toolUseId as string;
    const sub = new Writer(s, agentId, w.now);
    sub.ctx = 9_000;
    const saved = s.model;
    s.model = MODEL[spec.model ?? "sonnet"]!;
    const p = shape("user.prompt");
    for (const k of ["origin", "promptSource", "turnOrigin", "turnPosition", "permissionMode"]) delete p[k];
    sub.promptId = s.ids.uuid();
    p.promptId = sub.promptId;
    p.message = { role: "user", content: spec.prompt };
    sub.push(p, { parent: null });
    const result = spec.failure ?? spec.run(sub);
    if (!spec.failure) sub.say(result, { end: true });
    s.model = saved;
    s.sidecars.set(`subagents/agent-${agentId}.jsonl`, jsonl(sub.lines));
    s.sidecars.set(
      `subagents/agent-${agentId}.meta.json`,
      JSON.stringify({
        agentType: spec.type,
        isBuiltIn: !spec.type.includes(":"),
        description: spec.description,
        toolUseId,
        spawnDepth: 1,
        requestShape: "background",
        requestNonInteractive: true,
        model: spec.model ?? "sonnet",
        pickedEffortModel: MODEL[spec.model ?? "sonnet"],
        finished: true,
      }),
    );
    return { agentId, toolUseId, spec, endedAt: sub.now, result };
  });
}

/** The notification of a launched Subagent's end, delivered once the main clock has passed it. */
export function notify(w: Writer, a: Launched, gapMs = 20_000) {
  if (w.now < a.endedAt) w.clock.now = a.endedAt;
  return taskNotification(
    w,
    agentNotification(
      w.s,
      a.agentId,
      a.toolUseId,
      a.spec.description,
      a.spec.failure ? "failed" : "completed",
      a.result,
    ),
    gapMs,
  );
}

/* ───────────── Teammates (agent teams) ───────────── */

export interface TeammateSpec {
  name: string;
  color: string;
  type: string;
  description: string;
  prompt: string;
  /** The teammate's own work in its transcript; returns its last message. */
  run: (sub: Writer) => string;
}

export interface Spawned {
  name: string;
  agentId: string;
  toolUseId: string;
}

/** Spawns teammates from one message (`Agent` with a `name`); each gets an in-process transcript. */
export function spawnTeammates(w: Writer, team: string, specs: TeammateSpec[]): Spawned[] {
  const s = w.s;
  const ids = specs.map((t) => `a${t.name}-${s.ids.agent().slice(1)}`);
  const results = w.calls(
    specs.map((t, i) => {
      const r = shape("user.tool_result.Agent.teammate").toolUseResult;
      Object.assign(r, {
        status: "teammate_spawned",
        prompt: t.prompt,
        agentId: ids[i],
        agent_id: ids[i],
        resolvedModel: "claude-opus-5-5",
        teammate_id: `${t.name}@${team}`,
        agent_type: t.type,
        model: "opus",
        name: t.name,
        color: t.color,
        tmux_session_name: "in-process",
        tmux_window_name: "in-process",
        tmux_pane_id: "in-process",
        team_name: team,
        is_splitpane: false,
        plan_mode_required: false,
      });
      return {
        name: "Agent",
        input: {
          description: t.description,
          subagent_type: t.type,
          model: "opus",
          name: t.name,
          prompt: t.prompt,
          run_in_background: "true",
        },
        out: {
          content: [
            {
              type: "text",
              text: `Spawned successfully. (This tool result is internal metadata — never quote or paste any part of it, including the ID below, into a user-facing reply.)\nagent_id: ${ids[i]}\nname: ${t.name}\nThe agent is now running and will receive instructions via mailbox.`,
            },
          ],
          toolUseResult: r,
          ms: 400,
        },
      };
    }),
  );
  return specs.map((t, i) => {
    const agentId = ids[i]!;
    const sub = new Writer(s, agentId, w.now);
    const p = shape("user.prompt");
    for (const k of ["origin", "promptSource", "turnOrigin", "turnPosition", "permissionMode"]) delete p[k];
    sub.promptId = s.ids.uuid();
    p.promptId = sub.promptId;
    p.message = {
      role: "user",
      content: `<teammate-message teammate_id="team-lead" summary="${t.description}">\n${t.prompt}\n</teammate-message>`,
    };
    sub.push(p, { parent: null });
    sub.say(t.run(sub), { end: true });
    s.sidecars.set(`subagents/agent-${agentId}.jsonl`, jsonl(sub.lines));
    s.sidecars.set(
      `subagents/agent-${agentId}.meta.json`,
      JSON.stringify({
        agentType: t.name,
        isBuiltIn: false,
        description: t.description,
        name: t.name,
        spawnDepth: 0,
        requestShape: "background",
        requestNonInteractive: true,
        model: "opus",
        pickedEffortModel: "opus",
        taskKind: "in_process_teammate",
        teamName: team,
        color: t.color,
        planModeRequired: false,
        customAgentType: t.type,
        spawnedAgentType: t.type,
        permissionMode: "default",
      }),
    );
    return { name: t.name, agentId, toolUseId: results[i]!._toolUseId as string };
  });
}

/* ───────────── Workflows ───────────── */

export interface WorkflowSpec {
  name: string;
  summary: string;
  args: Record<string, unknown>;
  phases: Array<{ title: string; detail: string }>;
  agents: Array<{ label: string; phase: number; failed?: boolean; prompt: string; run: (sub: Writer) => string }>;
  result: unknown;
}

export interface WorkflowLaunch {
  taskId: string;
  toolUseId: string;
  spec: WorkflowSpec;
  endedAt: number;
}

/** Runs a Workflow in the background: the call, `workflows/<runId>.json`, its agents' transcripts and journal. */
export function workflow(w: Writer, spec: WorkflowSpec): WorkflowLaunch {
  const s = w.s;
  const runId = `wf_${s.ids.agent().slice(1, 9)}-${s.ids.agent().slice(1, 4)}`;
  const taskId = `w${s.ids.shell().slice(1)}`;
  const base = `/Users/dev/.claude/projects/-Users-dev-code-${s.o.project}/${s.o.id}`;
  const transcriptDir = `${base}/subagents/workflows/${runId}`;
  const scriptPath = `${base}/workflows/scripts/${spec.name}-${runId}.js`;
  const tur = shape("user.tool_result.Workflow").toolUseResult;
  Object.assign(tur, {
    runId,
    scriptPath,
    status: "async_launched",
    summary: spec.summary,
    taskId,
    taskType: "local_workflow",
    transcriptDir,
    workflowName: spec.name,
  });
  const r = w.call(
    "Workflow",
    { name: spec.name, args: spec.args },
    {
      content: `Workflow launched in background. Task ID: ${taskId}\nSummary: ${spec.summary}\nTranscript dir: ${transcriptDir}\nScript file: ${scriptPath}\nYou will be notified when it completes.`,
      toolUseResult: tur,
      ms: 600,
    },
  );
  const start = w.now;
  const progress: Entry[] = spec.phases.map((p, i) => ({ index: i + 1, title: p.title, type: "workflow_phase" }));
  const journal: Entry[] = [];
  let end = start;
  let tokens = 0;
  spec.agents.forEach((a, i) => {
    const agentId = s.ids.agent();
    const sub = new Writer(s, agentId, start + i * 1500);
    const p = shape("user.prompt");
    for (const k of ["origin", "promptSource", "turnOrigin", "turnPosition", "permissionMode"]) delete p[k];
    sub.promptId = s.ids.uuid();
    p.promptId = sub.promptId;
    p.message = { role: "user", content: a.prompt };
    sub.push(p, { parent: null });
    const result = a.run(sub);
    if (!a.failed) sub.say(result, { end: true });
    s.sidecars.set(`subagents/workflows/${runId}/agent-${agentId}.jsonl`, jsonl(sub.lines));
    s.sidecars.set(
      `subagents/workflows/${runId}/agent-${agentId}.meta.json`,
      JSON.stringify({ agentType: "workflow-subagent", spawnDepth: 1 }),
    );
    journal.push({ agentId, key: a.label, type: "started" });
    journal.push(
      a.failed
        ? { agentId, key: a.label, type: "failed", error: result }
        : { agentId, key: a.label, type: "completed", result },
    );
    const t = 40_000 + i * 9_000;
    tokens += t;
    end = Math.max(end, sub.now);
    progress.push({
      agentId,
      attempt: 1,
      durationMs: sub.now - (start + i * 1500),
      index: i + 1,
      label: a.label,
      model: "claude-sonnet-5-5",
      phaseIndex: a.phase,
      phaseTitle: spec.phases[a.phase - 1]!.title,
      promptPreview: a.prompt.slice(0, 120),
      resultPreview: result.slice(0, 120),
      startedAt: start + i * 1500,
      state: a.failed ? "failed" : "done",
      tokens: t,
      toolCalls: 3,
      type: "workflow_agent",
    });
  });
  s.sidecars.set(`subagents/workflows/${runId}/journal.jsonl`, jsonl(journal));
  s.sidecars.set(
    `workflows/${runId}.json`,
    JSON.stringify({
      agentCount: spec.agents.length,
      defaultModel: "claude-sonnet-5-5",
      durationMs: end - start,
      logs: [],
      phases: spec.phases,
      result: spec.result,
      runId,
      script: `export const meta = { name: "${spec.name}" }`,
      scriptPath,
      startTime: start,
      status: spec.agents.some((a) => a.failed) ? "failed" : "completed",
      summary: spec.summary,
      taskId,
      timestamp: new Date(end).toISOString(),
      totalTokens: tokens,
      totalToolCalls: spec.agents.length * 3,
      workflowName: spec.name,
      workflowProgress: progress,
    }),
  );
  return { taskId, toolUseId: r._toolUseId as string, spec, endedAt: end };
}

export function workflowNotification(w: Writer, l: WorkflowLaunch, gapMs = 20_000) {
  if (w.now < l.endedAt) w.clock.now = l.endedAt;
  const s = w.s;
  const file = `/private/tmp/claude-501/-Users-dev-code-${s.o.project}/${s.o.id}/tasks/${l.taskId}.output`;
  return taskNotification(
    w,
    `<task-notification>\n<task-id>${l.taskId}</task-id>\n<tool-use-id>${l.toolUseId}</tool-use-id>\n<output-file>${file}</output-file>\n<status>completed</status>\n<summary>Dynamic workflow "${l.spec.summary}" completed</summary>\n<result>${JSON.stringify(l.spec.result)}</result>\n</task-notification>`,
    gapMs,
  );
}
