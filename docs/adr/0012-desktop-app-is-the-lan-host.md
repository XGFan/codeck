# Mac 上由 CoDeck 应用自己当局域网 Host，不起常驻 daemon

在 Mac 上 Connect 一个 Session 后，想换到 iPhone 接着操作，得先让桌面放开它：ccrpc 对每个 claude 进程只允许一个连接，桌面和 daemon 是两个 Host，各起各的 ccrpc，后连的一方报「已有 ccrpc 连着这个 Session」。CONTEXT.md 里一个 Session 在一个 Host 上至多有一个 Connect、该 Host 的客户端共用，所以要做的是让桌面窗口和 iPhone 成为同一个 Host 的客户端。

因此 CoDeck 应用在设置的「局域网访问」打开时，在自己的进程里提供和 daemon 相同的 HTTP + WebSocket 接口（只暴露 `remote` 方法），iPhone 直接连它，不用改。开关默认关闭，切换后立即生效。开关打开时关窗只隐藏，⌘Q 才退出。修订 ADR-0011 里桌面只在进程内调用、不对外提供接口的部分；daemon 仍是 Linux 上的 Host。

## Considered Options

- **Mac 桌面改做常驻 daemon 的客户端**：关掉应用后 iPhone 仍可用，但 ADR-0011 拒绝它的理由还在：要管理 daemon 的安装、版本握手与升级重启。
- **保留两个 Host，Connect 在它们之间转交**：一端 Connect 时让另一端自动 Disconnect。架构改动小，但同一时刻只有一端能操作，两个 Host 之间要新增通信，索引也还是两份。
- **桌面前端改读 Host 的 ConnState，只留 Rust 一份折叠逻辑**：是 ADR-0011 设想的终态，但要重写前端的 Connect store，全部跑在 wire 回放上的 E2E 也要改造。这次不做，桌面继续自己折叠，并认领 Host 上的所有连接。
- **开关再加配对码或 token**：带着电脑换网络时也安全，但 daemon、iOS 与 WKWebView 都要改，这次不做。
- **始终监听，不设开关**：连上别人的网络时也会暴露，见 Consequences。

## Consequences

- 没有鉴权：开关打开时，同一网段的任何设备都能发提示词、作答审批、新建 Session，等于能在这台 Mac 上执行命令。咖啡馆 WiFi 也是私网地址，来源子网校验挡不住，所以只在可信网络里打开。
- iPhone 只在应用运行时可用；⌘Q 会 Disconnect 所有连接，Session 照常运行。
- 应用不阻止系统休眠：Mac 睡着后 iPhone 连不上 Host，要靠系统设置或 caffeinate 保持唤醒。
- Rust 与 TS 继续各有一份 Connect 状态逻辑，改一边要对照另一边。桌面的 Host 不管开关开没开都折叠连接状态，这样打开开关时已有的连接也能给 iPhone 用；只有 `ccrpc ls` 的常驻随开关启停，开关关闭时启动仍不读登录 shell。
- `Info.plist` 禁用了 App Nap，否则窗口隐藏后 iPhone 的响应会被节流；这是打包时定的，不随开关变化。
- daemon 和应用都默认用端口 38471，所以在同一台 Mac 上同时开时后启动的那个绑定失败；调试用的 daemon 要另开端口，且它仍是另一个 Host，同样受 ccrpc 单连接的限制。
