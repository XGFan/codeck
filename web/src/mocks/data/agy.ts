import type { Node } from "@/ipc/bindings";
import { asstNode, call, MIN, text, userNode } from "./builders";

/** Like the backend, which serializes tool input with sorted keys: `agyOriginal` comes first. */
const agyCall: typeof call = (toolUseId, name, input, result, o) => {
  const { agyOriginal, ...rest } = input as Record<string, unknown>;
  return call(toolUseId, name, agyOriginal === undefined ? rest : { agyOriginal, ...rest }, result, o);
};

export const AGY_LIVE_ID = "a6a10001-0012-4000-8000-000000000012";
export const AGY_LIVE_TITLE = "agy：检查限流中间件的边界用例";

export const AGY_ENDED_ID = "a6a10002-0013-4000-8000-000000000013";
export const AGY_ENDED_TITLE = "agy：整理 Flux 部署清单说明";

export const AGY_WORKER_WEB_ID = "a6a10003-0014-4000-8000-000000000014";
export const AGY_WORKER_WEB_TITLE = "Agy Session Frontend And E2E";

export const AGY_WORKER_LIVE_ID = "a6a10004-0015-4000-8000-000000000015";
export const AGY_WORKER_LIVE_TITLE = "Agy Live And Haa Integration";

export const AGY_WORKER_UNLINKED_ID = "a6a10005-0016-4000-8000-000000000016";
export const AGY_WORKER_UNLINKED_TITLE = "Agy Standalone Log Audit";

const RATE_LIMIT_PATH = "/Users/dev/Developer/lumen-api/src/middleware/rate_limit.rs";

export function agyLiveNodes(start: number): Node[] {
  const at = (m: number) => start + m * MIN;
  return [
    userNode("agy-l-u0", at(0), "检查一下 rate_limit.rs 里 burst 为 0 的边界处理，顺便补上单测。"),
    asstNode(
      "agy-l-a0",
      at(0.5),
      [
        text("先读一下限流中间件的实现。"),
        agyCall(
          "agy-l-tu1",
          "Read",
          {
            file_path: RATE_LIMIT_PATH,
            offset: 1,
            limit: 40,
            agyOriginal: {
              name: "read_file",
              args: { Path: RATE_LIMIT_PATH, StartLine: 1, EndLine: 40 },
            },
          },
          {
            text: [
              "     1\tpub fn check_burst(burst: u32) -> Result<(), RateLimitError> {",
              "     2\t    if burst == 0 {",
              "     3\t        return Ok(());",
              "     4\t    }",
              "     5\t    Ok(())",
              "     6\t}",
            ].join("\n"),
          },
        ),
        agyCall(
          "agy-l-tu2",
          "Edit",
          {
            file_path: RATE_LIMIT_PATH,
            old_string: "if burst == 0 {\n    return Ok(());\n}",
            new_string: "if burst == 0 {\n    return Err(RateLimitError::InvalidBurst);\n}",
            agyOriginal: {
              name: "edit_file",
              args: {
                Path: RATE_LIMIT_PATH,
                Target: "if burst == 0 {\n    return Ok(());\n}",
                Replacement: "if burst == 0 {\n    return Err(RateLimitError::InvalidBurst);\n}",
              },
            },
          },
          { text: "Edited file" },
        ),
        agyCall(
          "agy-l-tu3",
          "Bash",
          {
            command: "cargo test -p lumen-api rate_limit",
            description: "Run rate_limit unit tests",
            agyOriginal: {
              name: "run_command",
              args: {
                CommandLine: "cargo test -p lumen-api rate_limit",
                WaitMsBeforeAsync: 5000,
                toolSummary: "Run rate_limit unit tests",
              },
            },
          },
          { text: "running 6 tests\ntest result: ok. 6 passed; 0 failed" },
        ),
        text("已修复 `burst == 0` 返回 `RateLimitError::InvalidBurst`，6 个单元测试全部通过。"),
      ],
      { model: "gemini-4-neon-high" },
    ),
    userNode("agy-l-u1", at(3), "再确认一下有没有残留的后台任务。"),
    asstNode(
      "agy-l-a1",
      at(3.2),
      [
        agyCall("agy-l-tu4", "manage_task", { Action: "list" }, { text: "No running tasks" }),
        text("当前没有运行中的后台任务。"),
      ],
      { model: "gemini-4-neon-high" },
    ),
  ];
}

export function agyEndedNodes(start: number): Node[] {
  const at = (m: number) => start + m * MIN;
  return [
    userNode("agy-e-u0", at(0), "整理一下 Flux 部署清单里的镜像更新说明。"),
    asstNode(
      "agy-e-a0",
      at(0.5),
      [
        agyCall(
          "agy-e-tu1",
          "Read",
          {
            file_path: "/Users/dev/Developer/homelab-infra/clusters/prod/image-policy.yaml",
            agyOriginal: {
              name: "read_file",
              args: { Path: "/Users/dev/Developer/homelab-infra/clusters/prod/image-policy.yaml" },
            },
          },
          { text: "     1\tapiVersion: image.toolkit.fluxcd.io/v1beta2\n     2\tkind: ImagePolicy" },
        ),
        agyCall(
          "agy-e-tu2",
          "Bash",
          {
            command: "flux get images policy -A",
            description: "Inspect Flux image policies",
            agyOriginal: {
              name: "run_command",
              args: {
                CommandLine: "flux get images policy -A",
                WaitMsBeforeAsync: 5000,
                toolSummary: "Inspect Flux image policies",
              },
            },
          },
          { text: "flux-system\tkestrel-cli\t1.4.2" },
        ),
        text("已核对 `image-policy.yaml`，当前只跟踪 `1.x` 语义版本。"),
      ],
      { model: "gemini-4-neon-high" },
    ),
    userNode("agy-e-u1", at(5), "好的，这样就够了。"),
    asstNode("agy-e-a1", at(5.3), [text("收到，有需要再处理。")], { model: "gemini-4-neon-high" }),
  ];
}

export function agyWorkerWebNodes(start: number): Node[] {
  const at = (m: number) => start + m * MIN;
  return [
    userNode("agy-ww-u0", at(0), "把侧栏的 Worker 收进派活的 Session，补上单测与 E2E。"),
    asstNode(
      "agy-ww-a0",
      at(0.6),
      [
        agyCall(
          "agy-ww-tu1",
          "Bash",
          {
            command: "pnpm test web/src/features/sessions/groups.test.ts",
            description: "Run sidebar groups unit tests",
            agyOriginal: {
              name: "run_command",
              args: { CommandLine: "pnpm test web/src/features/sessions/groups.test.ts", WaitMsBeforeAsync: 5000 },
            },
          },
          { text: "12 passed" },
        ),
        text("已完成侧栏分组、头部返回行与 E2E 用例，所有测试通过。"),
      ],
      { model: "gemini-4-neon-high" },
    ),
  ];
}

export function agyWorkerLiveNodes(start: number): Node[] {
  const at = (m: number) => start + m * MIN;
  return [
    userNode("agy-wl-u0", at(0), "核对 Live 面板和 haa Worker 关联逻辑的边界情况。"),
    asstNode(
      "agy-wl-a0",
      at(0.5),
      [text("已确认 Live Worker 保留在 Live 区块中，未关联且非 Live 的 Worker 收进 Recent 末尾。")],
      { model: "gemini-4-neon-high" },
    ),
  ];
}

export function agyWorkerUnlinkedNodes(start: number): Node[] {
  const at = (m: number) => start + m * MIN;
  return [
    userNode("agy-wu-u0", at(0), "独立排查索引日志里的超时记录。"),
    asstNode("agy-wu-a0", at(0.5), [text("已扫描最近 7 天的索引日志，未发现异常超时。")], {
      model: "gemini-4-neon-high",
    }),
  ];
}
