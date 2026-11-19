import type { CodeOutcome, Node, ToolCall, ToolResult } from "@/ipc/bindings";
import { asstNode, call, MIN, text, userNode } from "./builders";

/**
 * A Code Mode Session (Claude writes JavaScript for the `REPL` tool): runs that return, throw, have a failed call,
 * and one still running — the shapes of crates/codeck-core/tests/fixtures/code_mode.
 */
export const CODE_MODE_ID = "c0de5e55-0011-4000-8000-000000000011";
export const CODE_MODE_TITLE = "Code Mode：把重试上限改成 5";

/** A call Claude's code made (no `kind`: it lives in the `REPL` call's code run). */
function inner(
  toolUseId: string,
  name: string,
  input: unknown,
  result: Partial<ToolResult> & { text: string },
): ToolCall {
  const { kind: _, ...c } = call(toolUseId, name, input, result) as Extract<
    ReturnType<typeof call>,
    { kind: "toolCall" }
  >;
  return c;
}

function repl(
  toolUseId: string,
  evalId: number,
  description: string,
  code: string,
  calls: ToolCall[],
  outcome: CodeOutcome | null,
) {
  return call(
    toolUseId,
    "REPL",
    { code, description },
    { text: `(dispatched #${evalId})` },
    { code: { calls, outcome } },
  );
}

const benchQuestion = {
  question: "跑哪些基准？",
  header: "基准",
  multiSelect: false,
  options: [
    { label: "只跑 retry", description: "重试路径" },
    { label: "全部", description: "约 20 分钟" },
  ],
};

const ok = (text: string, durationMs: number, log = ""): CodeOutcome => ({
  isError: false,
  text,
  log,
  truncated: false,
  durationMs,
});

const bash = (stdout: string) => JSON.stringify({ stdout, stderr: "", interrupted: false });
const patch = JSON.stringify({
  structuredPatch: [
    { oldStart: 2, oldLines: 1, newStart: 2, newLines: 1, lines: ["-    max_attempts: 3,", "+    max_attempts: 5,"] },
  ],
});

export function codeModeNodes(start: number): Node[] {
  const at = (m: number) => start + m * MIN;
  return [
    userNode("cm-u0", at(0), "把所有 max_attempts: 3 改成 5，跑一下重试测试，再读出配置里的重试参数。"),
    asstNode("cm-a0", at(0.2), [
      text("先找出所有写着 `max_attempts: 3` 的地方，一起改成 5，再跑重试测试。"),
      repl(
        "cm-r1",
        1,
        "把重试上限改成 5 并跑重试测试",
        [
          'const files = (await $`rg -l "max_attempts: 3" src`.text()).trim().split("\\n");',
          'await Promise.all(files.map((f) => edit(f, "max_attempts: 3", "max_attempts: 5")));',
          "const test = await $`cargo test --test retry`.nothrow();",
          "({ edited: files, exit: test.exitCode });",
        ].join("\n"),
        [
          inner(
            "cm-v1",
            "Bash",
            { command: 'rg -l "max_attempts: 3" src' },
            {
              text: "src/client/retry.rs\nsrc/middleware/upstream.rs",
              extraJson: bash("src/client/retry.rs\nsrc/middleware/upstream.rs"),
            },
          ),
          inner(
            "cm-v2",
            "Edit",
            { file_path: "src/client/retry.rs", old_string: "max_attempts: 3", new_string: "max_attempts: 5" },
            { text: "", extraJson: patch },
          ),
          inner(
            "cm-v3",
            "Edit",
            { file_path: "src/middleware/upstream.rs", old_string: "max_attempts: 3", new_string: "max_attempts: 5" },
            { text: "", extraJson: patch },
          ),
          inner(
            "cm-v4",
            "Bash",
            { command: "cargo test --test retry" },
            {
              text: "running 9 tests\ntest result: ok. 9 passed; 0 failed",
              extraJson: bash("running 9 tests\ntest result: ok. 9 passed; 0 failed"),
            },
          ),
        ],
        ok("{ edited: [ 'src/client/retry.rs', 'src/middleware/upstream.rs' ], exit: 0 }", 9123),
      ),
    ]),
    asstNode("cm-a1", at(0.6), [
      text("两处都改好了，重试测试 9 个全部通过。接着读配置。"),
      repl(
        "cm-r2",
        2,
        "读出配置里的重试参数",
        'const cfg = toml.parse(await file("config/default.toml").text());\ncfg.retry.max_attempts',
        [
          inner(
            "cm-v5",
            "Read",
            { file_path: "config/default.toml" },
            { text: "     1\t[server]\n     2\tport = 8080" },
          ),
        ],
        {
          isError: true,
          text: "TypeError: Cannot read properties of undefined (reading 'max_attempts')\n    at repl:2:11",
          log: "",
          truncated: false,
          durationMs: 30,
        },
      ),
    ]),
    asstNode("cm-a2", at(0.8), [
      text("配置里没有 retry 段。顺便并行跑一下 clippy。"),
      repl(
        "cm-r3",
        3,
        "并行跑 clippy 与重试测试",
        [
          "const [clippy, test] = await Promise.all([",
          "  $`cargo clippy --all-targets -- -D warnings`.nothrow(),",
          "  $`cargo test --test retry`.nothrow(),",
          "]);",
          "({ clippy: clippy.exitCode, test: test.exitCode });",
        ].join("\n"),
        [
          inner(
            "cm-v7",
            "Bash",
            { command: "cargo clippy --all-targets -- -D warnings" },
            { text: "Exit code 101\nerror: this `if` has identical blocks", isError: true },
          ),
          inner(
            "cm-v8",
            "Bash",
            { command: "cargo test --test retry" },
            { text: "test result: ok. 9 passed; 0 failed", extraJson: bash("test result: ok. 9 passed; 0 failed") },
          ),
        ],
        ok("{ clippy: 101, test: 0 }", 14_210, "clippy: 1 error"),
      ),
    ]),
    asstNode("cm-a3", at(1.2), [
      text("已完成：两处重试上限都改成 5，重试测试通过；clippy 在 `ratelimit.rs` 报了 1 个错。"),
    ]),
    userNode("cm-u1", at(3), "跑一下重试路径的基准测试。"),
    // A question asked from the code: its call is among the code's, its answer in `toolUseResult`.
    asstNode("cm-a6", at(3.02), [
      repl(
        "cm-r6",
        6,
        "问要跑哪些基准",
        "await tools.AskUserQuestion({ questions: [{ question: '跑哪些基准？', header: '基准', options: [...] }] })",
        [
          inner(
            "cm-v9",
            "AskUserQuestion",
            { questions: [benchQuestion] },
            {
              text: "",
              extraJson: JSON.stringify({ questions: [benchQuestion], answers: { "跑哪些基准？": "只跑 retry" } }),
            },
          ),
        ],
        ok("{ answers: { '跑哪些基准？': '只跑 retry' } }", 41_000),
      ),
    ]),
    // Rejected: the call failed and no code ran.
    asstNode("cm-a5", at(3.05), [
      call(
        "cm-r5",
        "REPL",
        { code: "await $`rm -rf target`", description: "清掉构建缓存" },
        { text: "The user doesn't want to proceed with this tool use.", isError: true },
        {
          code: {
            calls: [],
            outcome: {
              isError: true,
              text: "The user doesn't want to proceed with this tool use.",
              log: "",
              truncated: false,
              durationMs: null,
            },
          },
        },
      ),
    ]),
    asstNode("cm-a4", at(3.1), [
      repl("cm-r4", 4, "跑重试路径的基准测试", "await $`cargo bench --bench retry`", [], null),
    ]),
  ];
}
