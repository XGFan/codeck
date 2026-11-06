import { describe, expect, it } from "vitest";
import { nearlySame, peerMessages, previewOf, teammateItems, teammateParts } from "./teammate";

const msg = (attrs: string, body: string) => `<teammate-message ${attrs}>\n${body}\n</teammate-message>`;
const relayed = (...blocks: string[]) =>
  `Another Claude session sent a message:\n${blocks.join("\n\n")}\n\nThis came from another Claude session`;
const json = (o: object) => JSON.stringify(o);
const A = 'teammate_id="line-a" color="blue"';

describe("teammateParts", () => {
  it("the end of a round carries its result; without one it is just idle, or failed with the reason", () => {
    const done = json({ type: "idle_notification", from: "line-a", idleReason: "available", result: "Done.\n\n- a" });
    expect(teammateParts(relayed(msg(A, done)))).toEqual([
      { kind: "done", who: "line-a", color: "blue", result: "Done.\n\n- a", failed: false },
    ]);
    const idle = json({ type: "idle_notification", from: "line-a", idleReason: "available" });
    expect(teammateParts(relayed(msg(A, idle)))).toEqual([{ kind: "idle", who: "line-a", color: "blue" }]);
    const failed = json({
      type: "idle_notification",
      from: "line-a",
      idleReason: "failed",
      failureReason: "API Error",
    });
    expect(teammateParts(relayed(msg(A, failed)))).toEqual([
      { kind: "failed", who: "line-a", color: "blue", reason: "API Error" },
    ]);
  });

  it("its shutdown_approved and the system's teammate_terminated are one exit, in its color", () => {
    const parts = teammateParts(
      relayed(
        msg('teammate_id="system"', json({ type: "teammate_terminated", message: "line-a has shut down." })),
        msg(A, json({ type: "shutdown_approved", from: "line-a" })),
      ),
    );
    expect(parts).toEqual([{ kind: "exited", who: "line-a", color: "blue" }]);
  });

  it("adjacent reports form one part; a task assignment names who assigned it", () => {
    const parts = teammateParts(
      relayed(
        msg('teammate_id="line-a" summary="Stage one"', "Stage one done."),
        msg(A, "And the tests pass."),
        msg(
          'teammate_id="team-lead"',
          json({ type: "task_assignment", taskId: "6", subject: "Write docs", assignedBy: "team-lead" }),
        ),
      ),
    );
    expect(parts.map((p) => p.kind)).toEqual(["report", "task"]);
    const report = parts[0] as Extract<(typeof parts)[0], { kind: "report" }>;
    expect(report.messages).toHaveLength(2);
    expect(previewOf(report.messages[0]!)).toBe("Stage one");
    expect(previewOf(report.messages[1]!)).toBe("And the tests pass.");
    expect(parts[1]).toMatchObject({ who: "team-lead", taskId: "6", subject: "Write docs" });
  });

  it("an unknown JSON message stays a report, previewed by its type", () => {
    const parts = teammateParts(relayed(msg(A, json({ type: "plan_approval_request" }))));
    expect(parts[0]?.kind).toBe("report");
    expect(previewOf((parts[0] as { messages: Parameters<typeof previewOf>[0][] }).messages[0]!)).toBe(
      "plan_approval_request",
    );
  });
});

describe("peerMessages", () => {
  it("takes each message from another agent out of its wrapper, sender as its id", () => {
    const text =
      'Another Claude session sent a message:\n<cross-session-message from="uds:/tmp/1.sock" from-name="codeck-99">\nURGENT\n</cross-session-message>\n\nThis came from another Claude session';
    expect(peerMessages(text, null)).toEqual([{ id: "codeck-99", color: null, summary: null, body: "URGENT" }]);
    expect(peerMessages('<agent-message from="w2">\nhi\n</agent-message>', null)[0]).toMatchObject({
      id: "w2",
      body: "hi",
    });
  });

  it("a plugin's message is its text after the opening line", () => {
    expect(peerMessages("The plannotator plugin sent a message:\nFeedback.", "plannotator")).toEqual([
      { id: "plannotator", color: null, summary: null, body: "Feedback." },
    ]);
  });
});

describe("teammateItems", () => {
  const report = (body: string, summary?: string) =>
    msg(`teammate_id="bench" color="green"${summary ? ` summary="${summary}"` : ""}`, body);
  const done = (result: string) =>
    msg('teammate_id="bench" color="green"', json({ type: "idle_notification", from: "bench", result }));

  it("a report right before its sender's done goes into that card", () => {
    const items = teammateItems(
      teammateParts(
        relayed(
          report("| impl | p99 |\n|---|---|\n| redis | 7.2 ms |", "压测结果"),
          done("压测完成：Redis p99 7.2 ms，脚本在 scripts/bench.sh。"),
        ),
      ),
    );
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ kind: "done", who: "bench", report: { id: "bench", summary: "压测结果" } });
  });

  it("drops the report when the result says nearly the same", () => {
    const text = "Redis 计数完成，提交 7c41e2a，16 个测试全部通过。";
    const items = teammateItems(teammateParts(relayed(report(text), done(`${text}`))));
    expect(items).toEqual([expect.objectContaining({ kind: "done", report: null })]);
    expect(nearlySame("**Redis** 计数完成，提交 `7c41e2a`。", "Redis 计数完成，提交 7c41e2a")).toBe(true);
    expect(nearlySame("压测结果表", "Redis 计数完成，提交 7c41e2a")).toBe(false);
  });

  it("other reports stay cards of their own, one per message", () => {
    const other = msg('teammate_id="redis" color="blue"', "Redis 写好了");
    const items = teammateItems(
      teammateParts(relayed(other, report("第一条"), report("第二条"), done("完全不同的结论"))),
    );
    expect(items.map((i) => (i.kind === "report" ? `report:${i.message.id}:${i.message.body}` : i.kind))).toEqual([
      "report:redis:Redis 写好了",
      "report:bench:第一条",
      "done",
    ]);
    expect(items[2]).toMatchObject({ report: { body: "第二条" } });
  });
});
