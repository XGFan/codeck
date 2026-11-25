# CLAUDE.md

`desktop`：桌面应用的 Tauri 外壳，逻辑都在 `crates/codeck-host`。命令、数据流、跨模块的规则与约束见根目录的 [CLAUDE.md](../CLAUDE.md)。

- `desktop`：只剩外壳——生成命令、把 `HostEvent` 转成 tauri 事件、插件与窗口；退出时 `host.shutdown()`；「局域网访问」打开时关窗只隐藏（ADR-0012）。`codeck://` 链接（ADR-0014，deep-link 插件）：外壳只负责把窗口带到前面，解析与打开在前端（`lib/deepLink.ts`、`features/deep-link`，经 `Api.onOpenUrl`）；链接只导航，不加有副作用的链接。
