import { describe, expect, it } from "vitest";
import { parseDeepLink } from "./deepLink";

const id = "a7c63e92-0008-4000-8000-000000000008";

describe("parseDeepLink", () => {
  it("opens a Session", () => {
    expect(parseDeepLink(`codeck://session/${id}`)).toEqual({ kind: "session", sessionId: id });
    expect(parseDeepLink(`codeck://session/${id}/`)).toEqual({ kind: "session", sessionId: id });
    expect(parseDeepLink(`codeck://session/${id.toUpperCase()}?from=bot`)).toEqual({ kind: "session", sessionId: id });
  });

  it("ignores anything else", () => {
    for (const url of [
      "",
      "not a url",
      `https://session/${id}`,
      `codeck-dev://session/${id}`,
      `codeck://new/${id}`,
      "codeck://session/",
      "codeck://session/abc",
      `codeck://session/${id}/x`,
      `codeck://session/..%2F${id}`,
      `codeck:session/${id}`,
    ]) {
      expect(parseDeepLink(url), url).toBeNull();
    }
  });
});
