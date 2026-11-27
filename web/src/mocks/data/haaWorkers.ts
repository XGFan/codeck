import type { Node } from "@/ipc/bindings";
import { asstNode, call, MIN, NOW, text, userNode } from "./builders";

/**
 * Conversations of haa Workers that ran on codex and pi. Like the backend's, they are not Sessions of the list: the
 * mock reads them by id (`getTranscript`, `getToolDetail`), and `getSession` does not know them.
 */
export const CODEX_WORKER_ID = "01a21cbf-0000-7000-8000-0000000000c1";
export const PI_WORKER_ID = "haa-0000a001-0000-4000-8000-0000000000p1";

const start = NOW - 90 * MIN;
const at = (m: number) => start + m * MIN;

const codexNodes: Node[] = [
  userNode("cx-u0", at(0), "评审 haa 状态条的改动：只看 web/src/features/haa，找出会做错的地方。"),
  asstNode(
    "cx-a0",
    at(0.2),
    [
      call(
        "exec-cx-1",
        "Bash",
        { command: "git diff --stat HEAD~1 -- web/src/features/haa" },
        { text: " web/src/features/haa/HaaBar.tsx | 12 ++++++++----\n 1 file changed" },
      ),
    ],
    { model: "gpt-6.2-sol" },
  ),
  asstNode(
    "cx-a1",
    at(0.4),
    [
      call(
        "exec-cx-2#0",
        "Edit",
        { file_path: "/Users/dev/Developer/lumen-api/web/src/features/haa/HaaBar.tsx" },
        {
          text: "Updated /Users/dev/Developer/lumen-api/web/src/features/haa/HaaBar.tsx",
          extraJson: JSON.stringify({
            structuredPatch: [
              {
                oldStart: 10,
                oldLines: 1,
                newStart: 10,
                newLines: 1,
                lines: ["-  if (!listed) return;", "+  if (!open) return;"],
              },
            ],
          }),
        },
      ),
    ],
    { model: "gpt-6.2-sol" },
  ),
  asstNode(
    "cx-a2",
    at(0.6),
    [
      call(
        "call_cx_report",
        "mcp__haa__report",
        { status: "done", summary: "没有阻塞问题。" },
        { text: "Result 已提交" },
      ),
      text("评审完成：没有阻塞问题，已提交 Result。"),
    ],
    { model: "gpt-6.2-sol" },
  ),
];

const piNodes: Node[] = [
  userNode("pi-u0", at(10), "调研 pi 的 session 文件格式，只读。"),
  asstNode(
    "pi-a0",
    at(10.2),
    [
      call(
        "call_pi_read",
        "Read",
        { file_path: "/Users/dev/.pi/agent/sessions/README.md" },
        { text: "pi stores one JSONL file per session." },
      ),
      text("pi 的每个 session 是一个 JSONL 文件，条目之间用 id / parentId 连成树。"),
    ],
    { model: "deepseek-v4.1-flash" },
  ),
];

export const workerFiles = new Map<string, Node[]>([
  [CODEX_WORKER_ID, codexNodes],
  [PI_WORKER_ID, piNodes],
]);
