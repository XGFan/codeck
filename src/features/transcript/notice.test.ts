import { describe, expect, it } from "vitest";
import { describeNotice } from "./notice";

describe("describeNotice", () => {
  it("names the kind of background task from the summary", () => {
    expect(describeNotice('Background command "pnpm tauri build > b.log 2>&1" completed (exit code 0)', null)).toEqual({
      kind: "后台命令",
      name: "pnpm tauri build > b.log 2>&1",
      detail: null,
    });
    expect(describeNotice('Background command "cargo test" failed (exit code 101)', null)).toMatchObject({ kind: "后台命令", detail: "退出码 101" });
    expect(describeNotice('Monitor "paaa Worker 事件" script failed (exit 143)', null)).toMatchObject({ kind: "Monitor", detail: "退出码 143" });
    expect(describeNotice('Dynamic workflow "Map the repo" completed', null)).toMatchObject({ kind: "Workflow", name: "Map the repo" });
    expect(describeNotice('Agent "Review diff" failed: Agent terminated early due to an API error', null)).toEqual({
      kind: "后台 agent",
      name: "Review diff",
      detail: "Agent terminated early due to an API error",
    });
  });

  it("a linked agent run's description wins; anything else is a background task with its summary", () => {
    expect(describeNotice('Agent "Review diff" finished', "审查改动")).toEqual({ kind: "后台 agent", name: "审查改动", detail: null });
    expect(describeNotice("测试代理完成：新增 14 个用例", "写测试")).toEqual({ kind: "后台 agent", name: "写测试", detail: "测试代理完成：新增 14 个用例" });
    expect(describeNotice("MCP task k1 (github/list) failed.", null)).toEqual({ kind: "后台任务", name: null, detail: "MCP task k1 (github/list) failed." });
  });
});
