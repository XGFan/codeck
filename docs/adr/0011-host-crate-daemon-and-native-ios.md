# 业务逻辑放进不依赖 tauri 的 Host crate，iPhone 用原生客户端连 daemon

要从 iPhone 浏览 Session 并 Connect，最终的 Host 是没有图形界面的 Linux 主机（先在 Mac 上调试）。所以索引、监听、Live 轮询、ccrpc 与 Connect 状态都放进一个不依赖 tauri 的 crate，对外只有一张分发表 `dispatch(method, args)` 和一种事件信封 `{event, payload}`；事件都是快照或可以幂等重放的，客户端断线重连后先收一份当前快照，不保留事件历史、不补发；headless daemon 用 HTTP + WebSocket 暴露它，Mac 桌面用一个 Tauri 命令在进程内调它。前端只有一个 Api 实现，底下换传输。新增一项能力只改分发表，两端同时生效。

iPhone 端尽量做原生（之前做过「网页 + 壳」，体验很差）：列表、导航、输入框、审批与提问卡片用 SwiftUI；只读的 Transcript 用 WKWebView，复用桌面的渲染器。原生界面需要 Connect 状态，所以由 Host 把 ccrpc 的输出归约成状态，各客户端只收状态、发命令。这一点修订了 ADR-0001 在 iOS 上的取舍。

## Considered Options

- **桌面经 localhost HTTP 当独立 sidecar 的客户端**（feat/ios-mvp 的做法）：sidecar 是带隐藏窗口的 Tauri App，在 Linux 上跑不起来；健康检查不比版本，升级后桌面仍连着旧的 sidecar；桌面需要的设置、新建等接口也一起暴露给了局域网。
- **Mac 桌面也做 daemon 的客户端**：只有一种传输，但要在 Mac 上管理 daemon 的生命周期（launchd、版本握手、升级重启），眼下没有可见的好处。以后要连远程 Host 时，给桌面加一个「切换 Host」入口即可。
- **iPhone 用 Host 提供的网页**：没有版本错配，但体验差，见上。
- **Connect 状态由 Swift 再实现一份，或在 JavaScriptCore 里跑 TS**：前者让 ccrpc 协议一变就要改两处；后者的 Swift ↔ JS 桥接很别扭。同一 Host 的多个客户端共用一个 Connect，本来也需要 Host 持有状态。

## Consequences

- 一个 Session 在一个 Host 上至多有一个 Connect，该 Host 的所有客户端共用（CONTEXT.md「Host」）。跨 Host 的唯一性由 ccrpc 保证（第二个 attach 会报「已有 ccrpc 连着这个 Session」）。
- 在 Host 的 Connect 状态取代桌面 TS store 之前，Rust 与 TS 各有一份状态逻辑。ADR-0010 的撤回记录存在前端 localStorage 里，也要挪到 Host，否则 iPhone 认不出撤回的提示词。
- 方法表用一个宏定义，同时生成 Rust 的分发代码和由 specta 导出的 TS 方法类型；Swift 只手写它用到的那部分类型。每个方法标为 local（只给桌面，如改设置、在 Finder 里显示）或 remote，daemon 只暴露 remote 方法。daemon 的设置来自它自己的 settings.json 与命令行参数，客户端不能修改。
- `/health` 返回整数 `apiVersion`，只在接口有不兼容改动时加一；版本不一致时 iOS 直接说明，不等到解码失败。Transcript 的网页由 Host 提供，和 Host 的数据始终是同一版本，改渲染不用重装 App。
- MVP 只在局域网使用，不鉴权：daemon 默认只监听私网 IPv4、校验来源子网，回环地址只在用 `--listen` 显式指定时监听（开发与 E2E 用）；拒绝 `Origin: null` 和不认识的浏览器 Origin。新建、Resume 和 Fork 也对局域网开放，所以只能在可信网络里用。
- 在 Mac 上调试时，daemon 与桌面是两个 Host，索引、日志、图片各用自己的目录，不共用 `dev.joy.codeck` 的缓存。
- daemon 仍然遵守 ADR-0002，对数据根只读；Linux 上的状态目录按 XDG 规范放置。
