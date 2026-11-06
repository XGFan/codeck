import { describe, expect, it } from "vitest";
import { previewOf, teammateParts } from "./teammate";

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
