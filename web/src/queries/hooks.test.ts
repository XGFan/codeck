import { describe, expect, it } from "vitest";
import type { ImageRequest } from "@/ipc/bindings";
import { queryKeys } from "./hooks";

describe("queryKeys.image", () => {
  it("tells apart the images of parallel tool calls in one assistant node", () => {
    const req = (toolUseId: string): ImageRequest => ({
      sessionId: "s",
      scope: { kind: "main" },
      image: { nodeId: "n1", ordinal: 0, mediaType: "image/png", bytes: 1, toolUseId },
    });
    expect(queryKeys.image(req("tu_a"))).not.toEqual(queryKeys.image(req("tu_b")));
  });
});
