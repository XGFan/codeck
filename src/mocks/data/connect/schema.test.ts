import Ajv2020 from "ajv/dist/2020";
import { describe, expect, it } from "vitest";
import schemaText from "../../../../crates/ccrpc/docs/protocol.schema.json?raw";

// The recordings the mock ccrpc replays stand for what ccrpc prints and is sent: they must follow ccrpc's schema, or the
// front end is tested against a protocol that does not exist.
const recordings = import.meta.glob("./*.jsonl", { query: "?raw", import: "default", eager: true }) as Record<
  string,
  string
>;

const schema = JSON.parse(schemaText) as { $id: string };
const ajv = new Ajv2020({ strict: false, allErrors: true });
const validate = ajv.compile(schema);

/** Why `msg` is not valid: the errors against its own event's schema (the whole one is a `oneOf`, its errors are noise). */
function why(msg: { type?: string }): string {
  const own = ajv.getSchema(`${schema.$id}#/$defs/events/${msg.type}`);
  if (!own) return "no command, response or event of this type";
  own(msg);
  return (own.errors ?? []).map((e) => `${e.instancePath || "/"} ${e.message}`).join("; ");
}

describe("recordings follow ccrpc's protocol schema", () => {
  it("every sent and received line is valid", () => {
    const bad: string[] = [];
    for (const [path, text] of Object.entries(recordings)) {
      text.split("\n").forEach((line, i) => {
        if (!line.trim()) return;
        const wire = JSON.parse(line) as { dir: string; msg?: { type?: string } };
        if ((wire.dir !== "recv" && wire.dir !== "send") || !wire.msg) return;
        const msg: unknown = wire.msg;
        if (!validate(msg)) bad.push(`${path.replace("./", "")}:${i + 1} ${wire.msg.type}: ${why(wire.msg)}`);
      });
    }
    expect(bad).toEqual([]);
    expect(Object.keys(recordings).length).toBeGreaterThan(10);
  });
});
