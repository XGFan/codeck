// Fork Session：从主 Session 第一轮之后 fork 出来，换个思路。

import { prompt } from "../../scripts/demo/kit.ts";
import { type Session, forkSession } from "../../scripts/demo/lib.ts";

export function forkOf(main: Session): Session {
  const s = forkSession(
    {
      id: "e4f8b2a7-1c93-4d6e-8a05-3b7c9d1e2f60",
      project: "lumen-api",
      branch: "feat/rate-limit",
      start: "2026-11-04T09:30:00.000Z",
      title: "为 /v1/chat 接口增加令牌桶限流中间件 (Branch)",
      slug: "steady-token-bucket",
    },
    main,
    main.marks.get("afterTurn1")!,
  );
  const w = s.main;
  prompt(w, "换个思路：限流放到网关（nginx limit_req）做，应用里不管，比较一下两种做法。", 60_000);
  const t0 = w.now;
  w.say(
    "两种做法的对比：\n\n| | 网关（nginx `limit_req`） | 应用内中间件 |\n|---|---|---|\n| 按 API key 限 | 要把 key 放进 `$http_authorization` 再哈希 | 直接拿到 |\n| 多实例一致 | 每个 nginx 各算各的 | 接 Redis 后一致 |\n| 429 的响应体 | 只能是静态页 | 可以带 `Retry-After` 与 JSON |\n\n按 key 限流且要带 `Retry-After`，应用内更合适；网关可以再加一层粗粒度的总量保护。",
    { end: true },
  );
  w.endTurn(t0, 2);
  return s;
}
