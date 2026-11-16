// What the terminal window gets from its Pane's Holder: one tagged message per Holder frame
// (crates/codeck-term/src/holder.rs), the first byte the kind.

export type TermMessage =
  | { kind: "size"; cols: number; rows: number }
  /** The last lines of history (at most 2000), `\r\n`-separated, before the first snapshot. */
  | { kind: "history"; data: Uint8Array }
  /** The whole screen: clears it and redraws. */
  | { kind: "snapshot"; data: Uint8Array }
  | { kind: "output"; data: Uint8Array }
  /** The Pane's shell exited. */
  | { kind: "exit"; code: number }
  /** The connection to the Holder ended. */
  | { kind: "closed" };

export function parseMessage(buf: ArrayBuffer | Uint8Array): TermMessage | null {
  const u = buf instanceof Uint8Array ? buf : new Uint8Array(buf);
  if (u.length === 0) return null;
  const body = u.subarray(1);
  const view = new DataView(body.buffer, body.byteOffset, body.byteLength);
  switch (String.fromCharCode(u[0]!)) {
    case "S":
      return body.length < 4 ? null : { kind: "size", cols: view.getUint16(0, true), rows: view.getUint16(2, true) };
    case "B":
      return { kind: "history", data: body };
    case "P":
      return { kind: "snapshot", data: body };
    case "O":
      return { kind: "output", data: body };
    case "X":
      return body.length < 4 ? null : { kind: "exit", code: view.getInt32(0, true) };
    case "C":
      return { kind: "closed" };
    default:
      return null;
  }
}

/** Builds a message as the Holder side does (the mock uses it). */
export function message(m: TermMessage): Uint8Array {
  const tagged = (tag: string, body: Uint8Array) => {
    const out = new Uint8Array(body.length + 1);
    out[0] = tag.charCodeAt(0);
    out.set(body, 1);
    return out;
  };
  switch (m.kind) {
    case "size": {
      const b = new Uint8Array(4);
      new DataView(b.buffer).setUint16(0, m.cols, true);
      new DataView(b.buffer).setUint16(2, m.rows, true);
      return tagged("S", b);
    }
    case "history":
      return tagged("B", m.data);
    case "snapshot":
      return tagged("P", m.data);
    case "output":
      return tagged("O", m.data);
    case "exit": {
      const b = new Uint8Array(4);
      new DataView(b.buffer).setInt32(0, m.code, true);
      return tagged("X", b);
    }
    case "closed":
      return tagged("C", new Uint8Array());
  }
}
