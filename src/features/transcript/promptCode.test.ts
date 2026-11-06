import { describe, expect, it } from "vitest";
import { promptParts } from "./promptCode";

describe("promptParts", () => {
  it("leaves a prompt without backticks as one text part", () => {
    expect(promptParts("改一下 a*b* 和 # 标题\n- 列表")).toEqual([
      { kind: "text", text: "改一下 a*b* 和 # 标题\n- 列表" },
    ]);
  });

  it("splits inline code spans", () => {
    expect(promptParts("键名用 `[rate_limit] per_minute`，跑 ``a ` b`` 看看")).toEqual([
      { kind: "text", text: "键名用 " },
      { kind: "code", text: "[rate_limit] per_minute" },
      { kind: "text", text: "，跑 " },
      { kind: "code", text: "a ` b" },
      { kind: "text", text: " 看看" },
    ]);
  });

  it("keeps an unmatched backtick, and one that closes only on another line, as text", () => {
    expect(promptParts("按 ` 键\n然后 `x")).toEqual([{ kind: "text", text: "按 ` 键\n然后 `x" }]);
  });

  it("splits fenced blocks with their language, without the blank lines around them", () => {
    expect(promptParts("例如：\n\n```toml\n[rate_limit]\nper_minute = 120\n```\n\n照这个改 `config.rs`")).toEqual([
      { kind: "text", text: "例如：" },
      { kind: "block", text: "[rate_limit]\nper_minute = 120", lang: "toml" },
      { kind: "text", text: "照这个改 " },
      { kind: "code", text: "config.rs" },
    ]);
  });

  it("runs an unclosed fence to the end, and closes a fence only with as many backticks", () => {
    expect(promptParts("看这段报错\n````\nerror: ```\n````\n后面\n```\npanic at x")).toEqual([
      { kind: "text", text: "看这段报错" },
      { kind: "block", text: "error: ```", lang: null },
      { kind: "text", text: "后面" },
      { kind: "block", text: "panic at x", lang: null },
    ]);
  });

  it("treats triple backticks mid-line as an inline span, not a fence", () => {
    expect(promptParts("运行 ```ls -la``` 即可")).toEqual([
      { kind: "text", text: "运行 " },
      { kind: "code", text: "ls -la" },
      { kind: "text", text: " 即可" },
    ]);
  });
});
