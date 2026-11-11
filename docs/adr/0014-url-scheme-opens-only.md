# `codeck://` 链接只打开 Session，新建交给 CLI

别的工具（例如把请求转给 Claude Code 的转发器）想让 CoDeck 新建一个 Session，在自己那边留一条记录，之后点记录跳回 CoDeck 看这个 Session。我们注册 `codeck://` URL scheme（`tauri-plugin-deep-link`），v1 只有 `codeck://session/<sessionId>`：把窗口带到前面，选中这个 Session。新建仍走 `codeck new`（ADR-0013），它的输出多一个 `url` 字段，工具把它记下来就行。

链接只导航：不新建 Session、不发提示词、不 Connect 以外的任何写操作（自动 Connect 照设置）。任何网页、邮件或聊天消息都能放 `codeck://` 链接，浏览器的「打开 CoDeck？」点一下就送到应用，如果链接能带着提示词新建 Session，等于网页能借启动规则（可以带 `--dangerously-skip-permissions` 之类的参数）在这台 Mac 上执行命令，也绕过了 ADR-0013 守住的「审批停在 TUI 等人」与「拒绝 `!` 开头的提示词」。要创建就在本机调用 CLI，调用者已经能在本机执行命令，不扩大攻击面。

解析在前端：外壳的 `on_open_url` 只把窗口带到前面（「局域网访问」打开时关窗只是隐藏，ADR-0012），前端经 `Api.onOpenUrl` 收到链接、按严格格式解析（id 必须是 UUID，其他链接忽略，不提示），复用 ⌘K 与统计里打开 Session 的做法。mock 里用 `__cvMock.openUrl` 驱动同一个入口，所以 E2E 能覆盖。

## Considered Options

- **`codeck://new?cwd=…&prompt=…` 直接新建并发送**：工具不用调 CLI，但见上文，链接就成了执行命令的入口。
- **`codeck://new` 只预填「新建 Session」，由人按发送**：安全，但转发工具要的是自动创建，这个场景用不上，以后需要再加。
- **在 Rust 里解析、发类型化事件**：要新增命令与 bindings，冷启动时链接先于前端的监听到达，还得在 Rust 里缓存；插件的 `getCurrent()` 已经解决冷启动。
- **带 `?turn=N` 跳到某一轮**：轮次编号目前只在 CLI 里算（`turns.rs`），前端没有轮次到节点的映射，先不做；不认识的参数被忽略，以后加不会破坏旧链接。

## Consequences

- 只有打包好的应用才注册 scheme（macOS 在安装时从 Info.plist 读），`pnpm tauri dev` 收不到链接；真机验证要用构建出的 `.app`。用临时 BUNDLE_ID 构建的隔离版也会注册 `codeck://`，和装好的 CoDeck 同时存在时由 LaunchServices 挑一个。
- 插件的「当前链接」一直保留：前端重新载入时会再打开最后一个链接里的 Session。
- 链接指向创建时的那个 Session；在 Pane 里 `/clear` 或转到后台后，接续的 Session 要从对话里的接续入口过去（被接替的 Session 打开时照常转到接替者）。
- 刚 `codeck new` 完就点链接时，Session 可能还没进索引：在 `ccrpc ls` 里时显示「新 Session」，扫描中显示「正在更新索引」，索引收录后自动换成对话。
- iPhone 不注册；链接只按 sessionId 定位、不带 Host，以后 iOS App 注册同一 scheme 时可以沿用格式。
