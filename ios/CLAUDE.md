# CLAUDE.md

`ios`：iPhone App，经局域网浏览与 Connect 一台 Host（ADR-0011、ADR-0012）。命令、数据流、跨模块的规则与约束见根目录的 [CLAUDE.md](../CLAUDE.md)。

- SwiftUI，iOS 17+，`ios/project.yml` 用 xcodegen 生成 `CoDeck.xcodeproj`（生成物也提交；改了 project.yml 或增删文件后重新 `xcodegen generate`）。尽量原生：列表、导航、Session 页的悬浮顶栏与底部浮层、输入框、请求面板（审批/提问/选择）、后台任务都是 SwiftUI；只读的 Transcript 是 WKWebView 加载 Host 的 `/web/transcript.html`。
- `Sources/Host/`：地址（只收私网 IPv4，DEBUG 版另收 127.0.0.1）、`HostClient`（`/health` 校验 `apiVersion`、RPC）、`EventStream`（WebSocket；每个 `hello` 都整体重新同步；退到后台断开、回到前台重连）、`HostModel`；`Sources/Connect/` 是 Connect（状态来自 Host 的 `ConnState`，命令都经 `connect_command`）。文案与桌面一致（状态词、权限模式等），术语按 CONTEXT.md。
- 模拟器测试：`xcodebuild … test`；手动联调时 daemon 用 `--listen 127.0.0.1` 与独立的 `--state-dir`。
