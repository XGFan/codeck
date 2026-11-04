# cv-core test fixtures

Each directory here is a **mini data root** shaped like `~/.claude` (`projects/<encoded-cwd>/<sessionId>.jsonl`,
`<sessionId>/subagents/…`, `<sessionId>/workflows/…`, `<sessionId>/tool-results/…`, `sessions/`).
Tests copy a scenario into a temp dir with `common::fixture_root(name)` (see `tests/common/mod.rs`) and point the
scanner at it; never write into this tree. `tests/fixtures_sanity.rs` pins every number below.

All content is derived from real Claude Code 2.1.x transcripts and sanitized by `examples/sanitize.rs`:
structural keys (`type`, `uuid`, `parentUuid`, ids, `usage.*`, …) are kept, all other strings become deterministic
lorem filler (same input gives the same output, so verbatim resends stay equal), `signature` is dropped, base64
images are a 1x1 PNG, any home directory becomes `/Users/dev`, `cwd` is `/Users/dev/code/<project>`. Hand-written lines (marked
below) cover shapes the real data lacks. Rebuild a new sample with:

```
cargo run -p cv-core --example sanitize -- <file.jsonl|file.json> [--lines 1-50,70-80] [--uuids a,b | --uuid-file f] [--cwd /Users/dev/code/x] [--cap 160]
```

Counting rules used in the tables (independent of cv-core): *human prompts* = `user` entries that are not `isMeta`,
not `isCompactSummary`, whose `origin.kind` is absent or `human`, and whose content is not a tool result,
`<local-command…`, `<system-reminder…` or a relayed `<teammate-message…` (command prompts such as `<command-name>/release</command-name>` count);
*assistant msg ids* = distinct `message.id`; tokens use the max of each usage field per `message.id`
(`message_count` in the index = human prompts + assistant msg ids). Total size is far below 2 MB.

## basic

`basic/projects/-Users-dev-code-lumen-api/86d20d64-9fdd-5e64-aae4-93895b13068d.jsonl`, cwd `/Users/dev/code/lumen-api`.

| file | path under `projects/<dir>/` | lines | failed | uuids | dup uuids | human prompts | assistant msg ids | tool_use | tool errors | tokens in/out/cr/cc |
|---|---|---|---|---|---|---|---|---|---|---|
| main | `-Users-dev-code-lumen-api/86d20d64….jsonl` | 77 | 1 | 66 | 1 | 4 | 16 | 18 | 1 | 51 / 5151 / 1761742 / 33071 |

- Line 66 is the **one corrupt line** (truncated mid-write); every other line parses. `failed_lines = 1`, the next line still parses.
- Line 2 is a **stale `last-prompt`** (leaf not in the file); the last `last-prompt` (line 77) points at the final assistant fragment.
- Titles: `ai-title` = "Add retry to HTTP client", then `custom-title` = "retry-backoff" (custom wins). First human prompt: "Add retry with exponential backoff to the HTTP client in src/client.rs".
- **Unknown entry type** `hologram` (has a uuid and sits on the main line), **unknown block type** `hologram_block` inside an assistant message, **unknown system subtype** `future_notice`.
- Split fragments: 9 of the 16 assistant messages are split over 2-5 entries that repeat the same `usage` (count tokens once per `message.id`): e.g. a real thinking + text + tool_use(Bash) message in 3 fragments, a hand-written thinking + text pair, and the 5-fragment parallel group below.
- Tools (18 `tool_use`): Bash (ok, real), Read, Edit, Write, Grep (parallel group: one text fragment, several tool_use fragments, results hang off their own fragment), an `mcp__plugin_…state_list_active` call, Skill, AskUserQuestion, TaskCreate x2, TaskUpdate, SendMessage, WebFetch, **TodoWrite** (hand-written, `toolUseResult.newTodos`), and a **failed Bash** (hand-written, `is_error:true`, ANSI escapes in the output, `toolUseResult` is a string `Error: Exit code 101…`). Exactly one `is_error` tool_result.
- Hidden-by-default material: attachments (hook_success, skill_listing), a `isMeta` user entry (`<system-reminder>…`), system `turn_duration` and `stop_hook_summary`.
- Visible system entries: `informational`, `api_error`, and the unknown `future_notice`; the `local_command` entry is shown as a command output prompt.
- A command prompt `<command-message>release</command-message><command-name>/release</command-name><command-args>v1.2.0</command-args>` (title form "/release v1.2.0").
- One **duplicate uuid** (the `skill_listing` attachment is written twice with different `parentUuid`); first occurrence wins, `dup uuids = 1`.
- Other meta lines: `permission-mode`, `agent-name`, `mode`, `file-history-snapshot`, `queue-operation`.

## branch

`branch/projects/-Users-dev-code-orbit-web/6d4df888-616c-528f-b5b9-674beb27f393.jsonl` (hand-built with real entry shapes, no corrupt lines).

| file | path under `projects/<dir>/` | lines | failed | uuids | dup uuids | human prompts | assistant msg ids | tool_use | tool errors | tokens in/out/cr/cc |
|---|---|---|---|---|---|---|---|---|---|---|
| main | `-Users-dev-code-orbit-web/6d4df888….jsonl` | 23 | 0 | 21 | 0 | 7 | 8 | 1 | 0 | 40 / 804 / 160000 / 8000 |

Graph (indent = child, texts are unique so tests can find nodes by text):

```
"Write a function that parses ISO dates"  >  assistant  >  "Handle timezone offsets too"
  >  assistant(thinking) > assistant(tool_use Bash) > [tool_result, hook attachment]   (two children, NOT a branch)
  >  assistant "Tests pass; …"  >  system turn_duration  (satellite)
       |- "Now add unit tests"            > assistant "Added unit tests."                (abandoned head)
       `- "Add property tests instead"    > assistant "Added property-based tests."      (selected head, a rewind)
            |- "Run the whole suite"  (no reply, verbatim resend, dropped)
            `- "Run the whole suite"  > assistant "All property tests pass."          (selected)
                  > "Summarize the result"
                       |- assistant R1 (thinking + text "Summary, draft one.")          (regenerated, abandoned)
                       `- assistant R2 (thinking + text "Summary, draft two.")          (selected; last-prompt leaf)
```

Expected: Main Line leaf = the last `text` fragment of R2 (`last-prompt.leafUuid`); **2 BranchPoints** on the main line
(the rewind under the system satellite with options "Now add unit tests" / "Add property tests instead", and the
regenerated assistant message R1 / R2); the "Run the whole suite" pair yields **no** BranchPoint (hidden-resend rule: the
unanswered duplicate is dropped, fewer than 2 options remain); the tool_use fragment with a hook attachment next to
its tool_result must not produce a BranchPoint. Choosing the "Now add unit tests" head gives path root..system satellite, that prompt, its reply.

## compact

`compact/projects/-Users-dev-code-tidepool/`: three sessions (hand-built from real shapes; c3 mirrors a real file).

| file | path under `projects/<dir>/` | lines | failed | uuids | dup uuids | human prompts | assistant msg ids | tool_use | tool errors | tokens in/out/cr/cc |
|---|---|---|---|---|---|---|---|---|---|---|
| c1 sibling `/compact` | `-Users-dev-code-tidepool/699c395a….jsonl` | 13 | 0 | 12 | 0 | 4 | 4 | 0 | 0 | 20 / 480 / 80000 / 4000 |
| c2 missing logical parent | `-Users-dev-code-tidepool/d131117c….jsonl` | 11 | 0 | 10 | 0 | 3 | 4 | 0 | 0 | 20 / 480 / 80000 / 4000 |
| c3 cycle | `-Users-dev-code-tidepool/6a5f53ed….jsonl` | 14 | 0 | 13 | 0 | 4 | 4 | 0 | 0 | 20 / 480 / 80000 / 4000 |

- **c1** (`699c395a…`, amendment A1b): `X`(attachment) has two children, the `/compact` command prompt (`<command-name>/compact</command-name>`, then its `<local-command-stdout>`) and the `compact_boundary` (`parentUuid:null`, `logicalParentUuid = X`). Then `isCompactSummary` user, assistant, human prompt, assistant (last-prompt leaf). Expect: no BranchPoint, Main Line continuous through boundary + summary (the `/compact` pair is an off-path sibling), boundary rendered as divider, summary as collapsed card.
- **c2** (`d131117c…`, amendment A1a): the boundary's `logicalParentUuid` is **not in the file**; effective parent falls back to the nearest preceding uuid-bearing entry (the attachment `X`). Line 1 is a stale `last-prompt` (leaf missing), so the leaf is the newest entry. Expect: Main Line covers all 4 turns, one divider.
- **c3** (`6a5f53ed…`): real-world shape. The boundary's `logicalParentUuid` points at the **tail of the preserved segment, which is written after the boundary and descends from it** (boundary > summary > preserved assistant > attachment T > caveat(isMeta) > `/compact` prompt > stdout > prompt > assistant). Following `logicalParentUuid` therefore creates a **parent cycle**; assembly must terminate (treat the boundary as the root of the main line, never loop). Expect: finite path from the last assistant back to the boundary; no crash or hang.

## fork

`fork/projects/-Users-dev-code-tidepool/`: origin `d3b58148…` and child `d6ac0c3a…` (real EasyTier-style fork: first 69 lines of a real origin, the child repeats every uuid-bearing entry with `forkedFrom`).

| file | path under `projects/<dir>/` | lines | failed | uuids | dup uuids | human prompts | assistant msg ids | tool_use | tool errors | tokens in/out/cr/cc |
|---|---|---|---|---|---|---|---|---|---|---|
| origin | `-Users-dev-code-tidepool/d3b58148….jsonl` | 73 | 0 | 53 | 0 | 2 | 5 | 3 | 0 | 13 / 6337 / 152651 / 44466 |
| child | `-Users-dev-code-tidepool/d6ac0c3a….jsonl` | 73 | 0 | 53 | 0 | 2 | 5 | 3 | 0 | 13 / 6337 / 152651 / 44466 |

- Child: 51 uuid-bearing lines (53 uuids in total: + its own prompt and reply) carry `forkedFrom:{sessionId: origin, messageUuid: <own uuid>}` (inherited); real child files keep `session_id` = origin id inside assistant/system entries. The child's own entries have no `forkedFrom`: prompt "Try the daemon approach instead and compare" (parent = the last inherited entry), assistant reply, `custom-title` "Mac GUI architecture (Branch)", `last-prompt`.
- Origin continues independently from the same entry with "Which of the three options is the simplest to ship?", `ai-title` "Mac GUI architecture".
- Both share the same `root_uuid` (first uuid-bearing line); origin `created_ms` is not later than the child's. The inherited part has 1 human prompt, Agent tool calls (async, no sidecar files in this scenario) and a task-notification.

## copies

`copies/projects/`: the same session ids in two project dirs, plus (added by the helper) a symlinked project dir.

| project dir | session | lines | uuids | tokens in/out/cr/cc |
|---|---|---|---|---|
| `-Users-dev-code-kestrel-cli-old` | `2000072b…` | 5 | 4 | 10 / 240 / 40000 / 2000 |
| `-Users-dev-code-kestrel-cli-old` | `d7b7f7c0…` | 4 | 4 | 10 / 240 / 40000 / 2000 |
| `-Users-dev-code-kestrel-cli` | `2000072b…` | 5 | 4 | 10 / 240 / 40000 / 2000 |
| `-Users-dev-code-kestrel-cli` | `d7b7f7c0…` | 9 | 8 | 20 / 480 / 80000 / 4000 |

- `2000072b…` is byte-identical in `-Users-dev-code-kestrel-cli` and `-Users-dev-code-kestrel-cli-old`.
- `d7b7f7c0…`: `-old` holds a **byte prefix** (2 turns, 4 lines, no `last-prompt`), `-Users-dev-code-kestrel-cli` the **superset** (4 turns + `last-prompt`). Expect: one session in the list, primary = the larger file, union by uuid = 8 uuids, 4 human prompts.
- `fixture_root("copies")` additionally creates `projects/-Users-dev-code-kestrel-cli-link -> -Users-dev-code-kestrel-cli` (relative symlink). Expect: the linked dir is skipped (dedupe by realpath), so no extra projects or sessions.

## subagents

`subagents/projects/-Users-dev-code-tidepool/ee98ea57-9b48-56b0-9860-5c8105c7f967.jsonl` + `<sid>/subagents/`.

| file | path under `projects/<dir>/` | lines | failed | uuids | dup uuids | human prompts | assistant msg ids | tool_use | tool errors | tokens in/out/cr/cc |
|---|---|---|---|---|---|---|---|---|---|---|
| main | `-Users-dev-code-tidepool/ee98ea57….jsonl` | 18 | 0 | 17 | 0 | 1 | 4 | 3 | 0 | 10 / 13244 / 346381 / 100767 |

Main line: 1 prompt, then 3 Agent calls (tool_use: 3), a task-notification prompt and a closing assistant message. Hand-written tail (shape of real 2.1.207 lines): a relayed teammate message from `gui-impl` (`Another Claude session sent a message:\n<teammate-message teammate_id="gui-impl" color="red" summary="…">…</teammate-message>` plus the injected notice; not a human prompt) and a system `agents_killed` entry (envelope only, no `content`). Sidecar files (`agent-<id>.jsonl` + `.meta.json`, all lines `isSidechain:true`, `agentId` = id):

| agent id | file lines | role | link |
|---|---|---|---|
| `a11b19d522e5e3835` | 7 | **sync** Agent, result `status:"completed"` with `agentId` | `meta.toolUseId` = the main tool_use id |
| `a87b5e24025a157e4` | 14 | **async**: result `async_launched`, then a `task-notification` prompt (`<task-id>`, `<tool-use-id>`, `<status>`, `<summary>`) | `meta.toolUseId`; notification links by tool-use-id |
| `agui-impl-16f075d476ac2e52` | 18 | **teammate** (`taskKind:"in_process_teammate"`, `teamName`, `name:"gui-impl"`, no `toolUseId`); main result is `teammate_spawned` with `name:"gui-impl"` | `meta.name == toolUseResult.name` |
| `a1e2a06b87a0850ff` | 12 | **nested** under the teammate (`parentAgentId`, `spawnDepth:1`) | `meta.toolUseId` is a tool_use inside the teammate's file (line 15), not in main |
| `a0rphan000000000001` | 2 | **orphan**: hand-written, `toolUseId:"toolu_orphan000000000000001"` matches nothing | listed under "unlinked subagents" |

The agent transcripts are truncated real excerpts (they stop mid-conversation, which is fine for a viewer).

## workflow

`workflow/projects/-Users-dev-code-kestrel-cli/418be169-3e5e-5397-b9f9-862014ebf2fa.jsonl` + `<sid>/workflows/` + `<sid>/subagents/workflows/<runId>/`.

| file | path under `projects/<dir>/` | lines | failed | uuids | dup uuids | human prompts | assistant msg ids | tool_use | tool errors | tokens in/out/cr/cc |
|---|---|---|---|---|---|---|---|---|---|---|
| main | `-Users-dev-code-kestrel-cli/418be169….jsonl` | 11 | 0 | 10 | 0 | 1 | 3 | 2 | 0 | 9 / 16001 / 86971 / 170527 |

Main line: 1 prompt, two `Workflow` tool calls (results `status:"async_launched"` with `runId`, `taskId`, `transcriptDir`), closing message.

| run | `workflows/<runId>.json` | `subagents/workflows/<runId>/` |
|---|---|---|
| `wf_6e7e94b2-011` (completed) | yes: `status:"completed"`, 2 `workflow_phase` + 3 `workflow_agent` progress entries (labels core-loop, model-layer, tools; ids `a3e22cdf3b6a56db2`, `ad33e6d9c257a9b69`, `ad8527df83ea5bfde`), `totalTokens`, `durationMs` | 3 `agent-*.jsonl` (8 lines each) + `.meta.json` (`workflow-subagent`) + `journal.jsonl` (3 `started`, 3 `result`) |
| `wf_d83f7287-279` (still running) | **none** | 2 agents (6 lines each) + `journal.jsonl` (2 `started`, 1 `result`): build the run from the journal |

Only `agent-*.jsonl` count as agent transcripts; `journal.jsonl` and `*.meta.json` must not.

## persisted_and_images

`persisted_and_images/projects/-Users-dev-code-lumen-api/3e7603aa-503a-511b-8228-4e9155f1d5e4.jsonl` + `<sid>/tool-results/b8e6n2j5e.txt`.

| file | path under `projects/<dir>/` | lines | failed | uuids | dup uuids | human prompts | assistant msg ids | tool_use | tool errors | tokens in/out/cr/cc |
|---|---|---|---|---|---|---|---|---|---|---|
| main | `-Users-dev-code-lumen-api/3e7603aa….jsonl` | 16 | 0 | 15 | 0 | 2 | 6 | 3 | 0 | 1181 / 1527 / 631103 / 7634 |

- Two `<persisted-output>` tool results (Bash): the first points at `tool-results/b8e6n2j5e.txt` (shipped, 60 lines, 6 KB) with `toolUseResult.persistedOutputPath` = `/Users/dev/.claude/tool-results/b8e6n2j5e.txt` (absolute, must not be opened; only the basename counts) and `persistedOutputSize`; the second points at `zz9missing1.txt`, which is **not shipped** (missing-file path).
- Images: a Read tool result with an image inside `tool_result.content` plus `toolUseResult.file.base64`/`dimensions`, and a human prompt with a `text` + `image` block (`imagePasteIds`). All base64 data is the 1x1 PNG.

## live

`live/projects/-Users-dev-code-orbit-web/` (two tiny transcripts) and `live/sessions/*.json.template`.

| file | path under `projects/<dir>/` | lines | failed | uuids | dup uuids | human prompts | assistant msg ids | tool_use | tool errors | tokens in/out/cr/cc |
|---|---|---|---|---|---|---|---|---|---|---|
| busy | `-Users-dev-code-orbit-web/b5805323….jsonl` | 3 | 0 | 2 | 0 | 1 | 1 | 0 | 0 | 5 / 120 / 20000 / 1000 |
| shell | `-Users-dev-code-orbit-web/45470335….jsonl` | 2 | 0 | 2 | 0 | 1 | 1 | 0 | 0 | 5 / 120 / 20000 / 1000 |

`fixture_root("live")` renders the templates (placeholders `{{PID}}`, `{{PROC_START}}`, `{{PPID}}`, `{{PPID_START}}`, `{{DEAD_PID}}`, also in file names; `procStart` is `ps -o lstart=` output, e.g. `Wed Oct  7 09:34:56 2026`, local time; real Claude Code writes it in UTC, and the parser accepts either) so:

| file | session | status | process |
|---|---|---|---|
| `<test pid>.json` | `b5805323…` (transcript exists) | `busy` | alive, `procStart` matches: Live Busy |
| `<parent pid>.json` | `45470335…` (transcript exists) | `shell` | alive, `procStart` matches: Live Idle (raw status `shell` kept) |
| `<exited child pid>.json` | `6c8546d4…` (no transcript) | `idle` | dead (`kill(pid,0)` fails) |
| `1.json` | `1fc71c8b…` (no transcript) | `busy` | pid exists but `procStart` is `Mon Jan  1 00:00:00 2024`: pid reuse, not alive |

`sessions/4242.key` is a non-json file that scanners must ignore.
