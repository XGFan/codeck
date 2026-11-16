import { describe, expect, it } from "vitest";
import { initial, type ConnState, type Request } from "@/features/connect/conn";
import type { LsEntry } from "@/features/connect/protocol";
import { areaFor, BASE_FONT, fontFor } from "./fit";
import { message, parseMessage, type TermMessage } from "./messages";
import { termStatus } from "./status";
import { step } from "./windows";

const bytes = (...xs: number[]) => new Uint8Array(xs);

describe("parseMessage", () => {
  it("reads what the Holder side sends", () => {
    // S: u16 cols + u16 rows, little-endian.
    expect(parseMessage(bytes(0x53, 120, 0, 40, 0))).toEqual({ kind: "size", cols: 120, rows: 40 });
    expect(parseMessage(bytes(0x4f, 0xe4, 0xb8))).toEqual({ kind: "output", data: bytes(0xe4, 0xb8) });
    expect(parseMessage(bytes(0x58, 0xff, 0xff, 0xff, 0xff))).toEqual({ kind: "exit", code: -1 });
    expect(parseMessage(bytes(0x43))).toEqual({ kind: "closed" });
    expect(parseMessage(new Uint8Array([0x50, 0x41]).buffer)).toEqual({ kind: "snapshot", data: bytes(0x41) });
  });

  it("drops what it cannot read", () => {
    expect(parseMessage(bytes())).toBeNull();
    expect(parseMessage(bytes(0x53, 1))).toBeNull();
    expect(parseMessage(bytes(0x5a, 1))).toBeNull();
  });

  it("round-trips the mock's messages", () => {
    const all: TermMessage[] = [
      { kind: "size", cols: 300, rows: 2 },
      { kind: "history", data: bytes(1, 2) },
      { kind: "snapshot", data: bytes(3) },
      { kind: "output", data: bytes() },
      { kind: "exit", code: 3 },
      { kind: "closed" },
    ];
    for (const m of all) expect(parseMessage(message(m))).toEqual(m);
  });
});

describe("fit", () => {
  const cell = { width: 0.6, height: 1.2 };

  it("opens the window to fit the Pane at the base font", () => {
    const area = areaFor(120, 40, cell);
    expect(area).toEqual({ width: Math.ceil(120 * 0.6 * 13), height: Math.ceil(40 * 1.2 * 13) });
    expect(fontFor(120, 40, area.width, area.height, cell)).toBe(BASE_FONT);
  });

  it("follows the window, by the tighter side, in half pixels", () => {
    expect(fontFor(100, 30, 1200, 2000, cell)).toBe(20);
    expect(fontFor(100, 30, 2000, 400, cell)).toBe(11);
    expect(fontFor(120, 40, 500, 300, cell)).toBe(6);
    expect(fontFor(10, 5, 5000, 5000, cell)).toBe(28);
    expect(fontFor(0, 0, 500, 500, cell)).toBe(BASE_FONT);
  });
});

describe("termStatus", () => {
  const conn = (over: Partial<ConnState>): ConnState => ({ ...initial("c1", "h1"), ...over });
  const pane = (holder_id: string) => ({ holder_id }) as LsEntry;
  const screen = { type: "unrecognized", context: "x", lines: [], request_id: "unrecognized:x" } as unknown as Request;
  const dialog = {
    type: "dialog",
    waiting_for: "dialog open",
    lines: [],
    request_id: "dialog:x",
  } as unknown as Request;

  it("is the screen while ccrpc cannot read it", () => {
    expect(termStatus({ c1: conn({ state: "waiting", requests: [dialog] }) }, [], "h1")).toBe("screen");
    expect(termStatus({ c1: conn({ state: "idle", requests: [screen] }) }, [], "h1")).toBe("screen");
  });

  it("says Claude Code is back at its input box", () => {
    expect(termStatus({ c1: conn({ state: "idle" }) }, [], "h1")).toBe("input");
    expect(termStatus({ c1: conn({ state: "busy" }) }, [], "h1")).toBe("input");
    // Something else waits (an approval): neither.
    expect(termStatus({ c1: conn({ state: "waiting" }) }, [], "h1")).toBe("");
  });

  it("stays open on a Pane still running a Session, connected or not", () => {
    expect(termStatus({}, [pane("h1")], "h1")).toBe("");
    // Disconnected, or ccrpc ended: the Session runs on.
    expect(termStatus({ c1: conn({ procExit: { code: 0, stderr: "" } }) }, [pane("h1")], "h1")).toBe("");
  });

  it("closes once the Pane is gone", () => {
    expect(termStatus({}, [], "h1")).toBe("close");
    expect(termStatus({ c1: conn({ state: "idle" }) }, [pane("h1")], "h2")).toBe("close");
    expect(termStatus({ c1: conn({ sessionExited: true }) }, [], "h1")).toBe("close");
    // An ended connection beside a live one on the same Pane (after `claude --resume` in the terminal).
    const both = { c0: conn({ conn: "c0", sessionExited: true }), c1: conn({ state: "idle" }) };
    expect(termStatus(both, [], "h1")).toBe("input");
  });
});

describe("step", () => {
  it("keeps a window found after a reload until its Pane is seen live, then closes it when the Pane goes", () => {
    const found = { sent: null, seen: false };
    expect(step(found, "close")).toEqual({ close: false, send: false, next: found });
    expect(step(found, "screen")).toEqual({ close: false, send: true, next: { sent: "screen", seen: true } });
    expect(step({ sent: "screen", seen: true }, "close")).toEqual({ close: true });
  });

  it("sends a status only when it changed", () => {
    expect(step({ sent: "screen", seen: true }, "screen")).toMatchObject({ send: false });
    expect(step({ sent: "screen", seen: true }, "input")).toMatchObject({ send: true, next: { sent: "input" } });
  });
});
