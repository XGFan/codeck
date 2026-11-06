// Prompts, system entries and tool results in the shapes Claude Code 2.1.318 writes them (see demo/shapes).

import { type Entry, type Session, type ToolOut, type Writer, shape } from "./lib.ts";

/* ───────────── prompts ───────────── */

function promptEntry(w: Writer, kind: string, content: string | Entry[], fields: Entry = {}) {
  const e = shape(kind);
  e.message = { role: "user", content };
  Object.assign(e, fields);
  return w.push(e);
}

function snapshot(w: Writer, messageId: string) {
  const s = shape("file-history-snapshot");
  s.messageId = messageId;
  s.snapshot = { messageId, trackedFileBackups: {}, timestamp: new Date(w.now).toISOString() };
  w.meta(s);
}

function newPrompt(w: Writer) {
  w.promptId = w.s.ids.uuid();
  w.promptIndex++;
  w.turnIndex++;
  return { promptIndex: w.promptIndex, turnIndex: w.turnIndex };
}

/** A typed prompt (a turn start). */
export function prompt(w: Writer, text: string, gapMs = 60_000) {
  w.tick(gapMs);
  w.s.lastPrompt = text;
  const pos = newPrompt(w);
  const e = promptEntry(w, "user.prompt", text, {
    promptId: w.promptId,
    permissionMode: w.s.o.permissionMode ?? "default",
    origin: { kind: "human" },
    promptSource: "typed",
    turnOrigin: "human",
    turnPosition: pos,
  });
  snapshot(w, e.uuid);
  return e;
}

/** A prompt with a pasted screenshot. */
export function imagePrompt(w: Writer, text: string, pngBase64: string, gapMs = 60_000) {
  w.tick(gapMs);
  const pos = newPrompt(w);
  const e = promptEntry(
    w,
    "user.image",
    [
      { type: "text", text },
      { type: "image", source: { type: "base64", media_type: "image/png", data: pngBase64 } },
    ],
    {
      promptId: w.promptId,
      imagePasteIds: [1],
      permissionMode: w.s.o.permissionMode ?? "default",
      origin: { kind: "human" },
      promptSource: "typed",
      turnOrigin: "human",
      turnPosition: pos,
    },
  );
  snapshot(w, e.uuid);
  return e;
}

/** A custom slash command: the command prompt, then its expanded text as a meta entry (hidden). */
export function command(w: Writer, name: string, args: string, expanded: string, gapMs = 60_000) {
  w.tick(gapMs);
  w.s.lastPrompt = `/${name} ${args}`;
  const pos = newPrompt(w);
  const e = promptEntry(
    w,
    "user.command",
    `<command-message>${name}</command-message>\n<command-name>/${name}</command-name>\n<command-args>${args}</command-args>`,
    { promptId: w.promptId, origin: { kind: "human" }, turnOrigin: "human", turnPosition: pos },
  );
  snapshot(w, e.uuid);
  w.tick(50);
  const m = shape("user.meta");
  m.message = { role: "user", content: [{ type: "text", text: expanded }] };
  m.promptId = w.promptId;
  delete m.turnCompanion;
  w.push(m);
  return e;
}

/** A `!` shell command typed in the input box and its output. */
export function bang(w: Writer, cmd: string, stdout: string, gapMs = 60_000) {
  w.tick(gapMs);
  const pos = newPrompt(w);
  promptEntry(w, "user.bash_input", `<bash-input>${cmd}</bash-input>`, { promptId: w.promptId });
  w.tick(1500);
  promptEntry(w, "user.bash_stdout", `<bash-stdout>${stdout}</bash-stdout><bash-stderr></bash-stderr>`, {
    promptId: w.promptId,
    turnOrigin: "human",
    turnPosition: pos,
  });
}

/** A built-in command run by Claude Code itself (`/rename`…): two system entries. */
export function localCommand(w: Writer, name: string, args: string, stdout: string, gapMs = 30_000) {
  w.tick(gapMs);
  const contents = [
    `<command-name>/${name}</command-name>\n            <command-message>${name}</command-message>\n            <command-args>${args}</command-args>`,
    `<local-command-stdout>${stdout}</local-command-stdout>`,
  ];
  contents.forEach((content, i) => {
    const e = shape("system.local_command");
    Object.assign(e, { content, level: "info", isMeta: false });
    // The output line records which command it came from.
    if (i === 1) e.commandRun = { command: name, args };
    w.push(e);
    w.tick(200);
  });
}

/** A background task's end, delivered as a prompt when Claude is idle. */
export function taskNotification(w: Writer, body: string, gapMs = 30_000) {
  w.tick(gapMs);
  const pos = newPrompt(w);
  return promptEntry(w, "user.task_notification", body, {
    promptId: w.promptId,
    permissionMode: w.s.o.permissionMode ?? "default",
    origin: { kind: "task-notification", producer: "session-task", runId: w.s.ids.shell() },
    promptSource: "system",
    turnOrigin: "task_notification",
    turnPosition: pos,
    queueSkipAttachments: true,
  });
}

/** Messages a teammate relayed to the lead: `<teammate-message>` blocks. */
export function teammateMessage(
  w: Writer,
  messages: Array<{ id: string; color?: string; summary?: string; body: string }>,
  gapMs = 60_000,
) {
  w.tick(gapMs);
  const pos = newPrompt(w);
  const blocks = messages
    .map(
      (m) =>
        `<teammate-message teammate_id="${m.id}"${m.color ? ` color="${m.color}"` : ""}${m.summary ? ` summary="${m.summary}"` : ""}>\n${m.body}\n</teammate-message>`,
    )
    .join("\n\n");
  return promptEntry(w, "user.teammate", `Another Claude session sent a message:\n${blocks}`, {
    promptId: w.promptId,
    turnPosition: pos,
  });
}

/** A message another Claude session sent with SendMessage (recorded as meta, origin peer). */
export function peerMessage(w: Writer, fromName: string, body: string, gapMs = 60_000) {
  w.tick(gapMs);
  const pos = newPrompt(w);
  const sock = "uds:/tmp/cc-socks/41022.sock";
  return promptEntry(
    w,
    "user.peer",
    `Another Claude session sent a message:\n<cross-session-message from="${sock}" from-name="${fromName}">\n${body}\n</cross-session-message>`,
    {
      promptId: w.promptId,
      isMeta: true,
      permissionMode: w.s.o.permissionMode ?? "default",
      origin: {
        kind: "peer",
        from: sock,
        verifiedPeerPid: 41022,
        msg_id: w.s.ids.uuid(),
        name: fromName,
        hopChain: ["5f0c2a9e41d7b38c6a1e9d02"],
        fromMode: "default",
        body,
      },
      promptSource: "system",
      queueSkipSlashCommands: true,
      turnOrigin: "peer",
      turnPosition: pos,
      queueSkipAttachments: true,
    },
  );
}

/* ───────────── system entries ───────────── */

export function informational(w: Writer, text: string, level = "notice") {
  const e = shape("system.informational");
  Object.assign(e, { content: text, level, isMeta: false });
  return w.push(e);
}

/** An overloaded API request Claude Code retries (shown as a red 警告 line). */
export function apiRetry(w: Writer, attempt: number) {
  w.tick(4000);
  const req = w.s.ids.req();
  const e = shape("system.api_error");
  e.error = {
    message: `529 {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"},"request_id":"${req}"}`,
    status: 529,
    requestId: req,
    formatted: "529 Overloaded",
    connection: null,
    isNetworkDown: false,
    rateLimits: null,
  };
  Object.assign(e, { level: "error", maxRetries: 10, retryAttempt: attempt, retryInMs: 530 * 2 ** (attempt - 1) });
  return w.push(e);
}

/** The synthetic assistant message Claude Code writes when the retries run out. */
export function apiErrorMessage(w: Writer) {
  w.tick(2000);
  const e = shape("assistant.api_error");
  e.message.content = [
    {
      type: "text",
      text: "API Error: 529 Overloaded. This is a server-side issue, usually temporary — try again in a moment.",
    },
  ];
  e.message.id = w.s.ids.uuid();
  delete e.streamCutCause;
  delete e.truncatedAfterOutput;
  Object.assign(e, { error: "server_error", apiErrorStatus: 529, perTurnEffort: "high" });
  return w.push(e);
}

/** `/compact` or an auto-compact: the boundary (a new root) and the summary prompt after it. */
export function compact(w: Writer, summary: string, preTokens: number, trigger: "auto" | "manual" = "auto") {
  w.tick(45_000);
  const b = shape("system.compact_boundary");
  const logicalParent = w.last;
  b.content = "Conversation compacted";
  b.level = "info";
  b.isMeta = false;
  b.compactMetadata = {
    trigger,
    preTokens,
    durationMs: 41_820,
    postTokens: 24_310,
    cumulativeDroppedTokens: preTokens - 24_310,
  };
  w.push(b, { parent: null });
  b.logicalParentUuid = logicalParent;
  // Re-order so the logical parent sits next to parentUuid, as in real files.
  const last = w.lines.pop()!;
  const { parentUuid, logicalParentUuid, ...rest } = last;
  w.lines.push({ parentUuid, logicalParentUuid, ...rest });
  w.tick(10);
  const pos = newPrompt(w);
  const s = shape("user.compact_summary");
  s.message = {
    role: "user",
    content: `This session is being continued from a previous conversation that ran out of context. The summary below covers the earlier portion of the conversation.\n\n${summary}`,
  };
  Object.assign(s, {
    promptId: w.promptId,
    isCompactSummary: true,
    isVisibleInTranscriptOnly: true,
    turnPosition: pos,
  });
  w.push(s);
  w.ctx = 24_310;
}

/* ───────────── tool results ───────────── */

const ms = (n: number) => n;

export function bash(
  stdout: string,
  opts: { exit?: number; stderr?: string; interrupted?: boolean; ms?: number } = {},
): ToolOut {
  const failed = (opts.exit ?? 0) !== 0;
  const text = failed
    ? `Exit code ${opts.exit}\n${[stdout, opts.stderr].filter(Boolean).join("\n")}`
    : stdout || "(Bash completed with no output)";
  return {
    content: text,
    isError: failed || undefined,
    toolUseResult: failed
      ? `Error: ${text}`
      : {
          stdout,
          stderr: opts.stderr ?? "",
          interrupted: opts.interrupted ?? false,
          isImage: false,
          noOutputExpected: false,
        },
    ms: opts.ms ?? 2000,
    extra: { sandboxed: false },
  };
}

export function bashBackground(id: string, cwd: string): ToolOut {
  const file = `/private/tmp/claude-501/${cwd.replaceAll("/", "-")}/tasks/${id}.output`;
  return {
    content: `Command running in background with ID: ${id}. Output is being written to: ${file}. You will be notified when it completes — do not poll that file or sleep while waiting. Read the file only if the user asks for progress, or if you need a line it prints before you can go on, such as a server's ready line or port.`,
    toolUseResult: {
      stdout: "",
      stderr: "",
      interrupted: false,
      isImage: false,
      noOutputExpected: false,
      backgroundTaskId: id,
      backgroundCwdHint: cwd,
    },
    ms: ms(180),
    extra: { sandboxed: false },
  };
}

/** `patch`: hunks as Claude Code reports them (`lines` prefixed with " ", "-", "+"). */
export function edit(
  file: string,
  oldString: string,
  newString: string,
  patch: Array<{ oldStart: number; oldLines: number; newStart: number; newLines: number; lines: string[] }>,
  opts: { replaceAll?: boolean } = {},
): ToolOut {
  return {
    content: `The file ${file} has been updated successfully.`,
    toolUseResult: {
      filePath: file,
      oldString,
      newString,
      originalFile: null,
      structuredPatch: patch,
      userModified: false,
      replaceAll: opts.replaceAll ?? false,
      contentNotInModelContext: true,
    },
    ms: ms(900),
  };
}

export function editFailed(message: string): ToolOut {
  return {
    content: `<tool_use_error>${message}</tool_use_error>`,
    isError: true,
    toolUseResult: `Error: ${message}`,
    ms: ms(40),
  };
}

export function write(file: string, content: string): ToolOut {
  return {
    content: `File created successfully at: ${file}`,
    toolUseResult: {
      type: "create",
      filePath: file,
      content,
      structuredPatch: [],
      originalFile: null,
      userModified: false,
    },
    ms: ms(300),
  };
}

export function read(file: string, content: string, startLine = 1): ToolOut {
  const lines = content.replace(/\n$/, "").split("\n");
  return {
    content: lines.map((l, i) => `${startLine + i}\t${l}`).join("\n"),
    toolUseResult: {
      type: "text",
      file: { filePath: file, content, numLines: lines.length, startLine, totalLines: startLine + lines.length + 40 },
    },
    ms: ms(90),
  };
}

export function grep(lines: string[], files: string[]): ToolOut {
  return {
    content: lines.join("\n"),
    toolUseResult: {
      mode: "content",
      numFiles: files.length,
      filenames: files,
      content: lines.join("\n"),
      numLines: lines.length,
    },
    ms: ms(210),
  };
}

export function webFetch(url: string, result: string): ToolOut {
  return {
    content: result,
    toolUseResult: { bytes: 48_211, code: 200, codeText: "OK", result, durationMs: 3120, url },
    ms: ms(3200),
  };
}

export function webSearch(query: string, links: Array<{ title: string; url: string }>, summary: string): ToolOut {
  const content = `Web search results for query: "${query}"\n\nLinks: ${JSON.stringify(links)}\n\n${summary}`;
  return {
    content,
    toolUseResult: {
      query,
      results: [{ tool_use_id: "srvtoolu_01", content: links }, summary],
      durationSeconds: 5.8,
      searchCount: 1,
    },
    ms: ms(6000),
  };
}

export function mcp(text: string): ToolOut {
  return { content: [{ type: "text", text }], toolUseResult: [{ type: "text", text }], ms: ms(1800) };
}

export function taskCreate(id: string, subject: string): ToolOut {
  return {
    content: `Task #${id} created successfully: ${subject}`,
    toolUseResult: { task: { id, subject } },
    ms: ms(60),
  };
}

export function taskUpdate(id: string, from: string, to: string): ToolOut {
  return {
    content: `Updated task #${id} status`,
    toolUseResult: { success: true, taskId: id, updatedFields: ["status"], statusChange: { from, to } },
    ms: ms(60),
  };
}

export interface Question {
  question: string;
  header: string;
  multiSelect: boolean;
  options: Array<{ label: string; description: string }>;
}

export function askAnswered(questions: Question[], answers: Record<string, string>): ToolOut {
  const pairs = Object.entries(answers)
    .map(([q, a]) => `"${q}"="${a}"`)
    .join(", ");
  return {
    content: `Your questions have been answered: ${pairs}. You can now continue with the user's answers in mind.`,
    toolUseResult: { questions, answers },
    ms: ms(42_000),
  };
}

/* ───────────── background tasks ───────────── */

/** The launch result of a background Subagent (what the card links its notification by). */
export function agentLaunched(s: Session, agentId: string, description: string, promptText: string): ToolOut {
  const file = `/private/tmp/claude-501/-Users-dev-code-${s.o.project}/${s.o.id}/tasks/${agentId}.output`;
  return {
    content: [
      {
        type: "text",
        text: `Async agent launched successfully. (This tool result is internal metadata — never quote or paste any part of it, including the agentId below, into a user-facing reply.)\nagentId: ${agentId} (internal ID - do not mention to user. Use SendMessage with to: '${agentId}', summary: '<5-10 word recap>' to continue this agent.)\nThe agent is working in the background. You will be notified automatically when it completes. You know nothing about its results until that notification arrives — do not report, assume, or predict them; continue other work or respond to the user in the meantime.\nDo not duplicate this agent's work — avoid working with the same files or topics it is using.\noutput_file: ${file}\nDo NOT Read or tail this file via the shell tool — it is the full subagent JSONL transcript and reading it will overflow your context. If the user asks for progress, say the agent is still running; you'll get a completion notification.`,
      },
    ],
    toolUseResult: {
      isAsync: true,
      status: "async_launched",
      agentId,
      description,
      resolvedModel: "claude-sonnet-5-5",
      prompt: promptText,
      outputFile: file,
      canReadOutputFile: true,
      canContinueAgent: true,
    },
    ms: ms(240),
  };
}

export function agentNotification(
  s: Session,
  agentId: string,
  toolUseId: string,
  description: string,
  status: "completed" | "failed",
  result: string,
) {
  const file = `/private/tmp/claude-501/-Users-dev-code-${s.o.project}/${s.o.id}/tasks/${agentId}.output`;
  const summary =
    status === "completed"
      ? `Agent "${description}" finished`
      : `Agent "${description}" failed: ${result.split("\n")[0]}`;
  return `<task-notification>\n<task-id>${agentId}</task-id>\n<tool-use-id>${toolUseId}</tool-use-id>\n<output-file>${file}</output-file>\n<status>${status}</status>\n<summary>${summary}</summary>\n<note>A task-notification fires each time this agent stops with no live background children of its own. The user can send it another message and resume it, so the same task-id may notify more than once.</note>\n<result>${result}</result>\n</task-notification>`;
}

export function shellNotification(s: Session, id: string, toolUseId: string, description: string, exit: number) {
  const file = `/private/tmp/claude-501/-Users-dev-code-${s.o.project}/${s.o.id}/tasks/${id}.output`;
  const status = exit === 0 ? "completed" : "failed";
  return `<task-notification>\n<task-id>${id}</task-id>\n<tool-use-id>${toolUseId}</tool-use-id>\n<output-file>${file}</output-file>\n<status>${status}</status>\n<summary>Background command "${description}" ${exit === 0 ? "completed (exit code 0)" : `failed with exit code ${exit}`}</summary>\n</task-notification>`;
}
