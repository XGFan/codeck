// @vitest-environment node
import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";
import { createRemoteApi } from "./remote";

vi.stubGlobal("document", { addEventListener: () => {}, visibilityState: "visible" });

const bindings = readFileSync(new URL("./bindings.ts", import.meta.url), "utf8");
const re = /^\t(\w+): \(([^)]*)\) => __TAURI_INVOKE<.*?>\("(\w+)"(?:, \{ ([^}]*) \})?\)/gm;
const commands = [...bindings.matchAll(re)].map((m) => ({
  name: m[1],
  arity: m[2].trim() ? m[2].split(",").length : 0,
  method: m[3],
  keys: m[4] ? m[4].split(",").map((k) => k.trim()) : [],
}));

function stub(reply: { status?: number; body: unknown }) {
  const calls: { url: string; body: { method: string; args: Record<string, unknown> } }[] = [];
  vi.stubGlobal("fetch", async (url: string, init: { body: string }) => {
    calls.push({ url, body: JSON.parse(init.body) });
    return { ok: (reply.status ?? 200) < 400, status: reply.status ?? 200, json: async () => reply.body };
  });
  return calls;
}

describe("remote api", () => {
  it("finds the commands in bindings.ts", () => {
    expect(commands.length).toBeGreaterThan(30);
  });

  it("sends each command's method and arg keys", async () => {
    const api = createRemoteApi("http://h:1/") as unknown as Record<string, (...a: unknown[]) => Promise<unknown>>;
    // Finder-only commands are rejected locally, never sent.
    const local = new Set(["revealSessionFile", "ccrpcOpenAnalyzerRecords"]);
    for (const c of commands) {
      if (local.has(c.name)) continue;
      expect(typeof api[c.name], c.name).toBe("function");
      const calls = stub({ body: { result: null } });
      const args = Array.from({ length: c.arity }, () => (c.name === "ccrpcSend" ? {} : "x"));
      await api[c.name](...args);
      expect(calls, c.name).toHaveLength(1);
      expect(calls[0].url).toBe("http://h:1/v1/rpc");
      expect(calls[0].body.method, c.name).toBe(c.method);
      expect(Object.keys(calls[0].body.args).sort(), c.name).toEqual([...c.keys].sort());
    }
  });

  it("rejects with the error object", async () => {
    stub({ status: 404, body: { error: { code: "not_found", message: "没有这个 Session" } } });
    await expect(createRemoteApi("http://h:1").getSession("x")).rejects.toEqual({
      code: "not_found",
      message: "没有这个 Session",
    });
  });

  it("returns the result", async () => {
    stub({ body: { result: [1] } });
    await expect(createRemoteApi("http://h:1").listProjects()).resolves.toEqual([1]);
  });
});
