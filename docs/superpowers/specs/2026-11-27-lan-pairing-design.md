# 局域网访问：TLS 加扫码配对

iPhone 连 Host 现在是手输私网 IPv4、明文 HTTP、不鉴权（ADR-0011、ADR-0012）：同一网段的任何设备都能读 Session、发提示词、新建 Session，公共 Wi-Fi 上还能被抓包。这次给局域网访问加上加密与鉴权，并用扫码代替手输地址。

## 目标与边界

- 防冒用也防偷听：局域网上一律 TLS，每台设备凭自己的 token 访问。
- 首次配对扫码：二维码带地址、端口、证书指纹与一次性配对密钥，iPhone 不用信任首次连接。
- 不做：Bonjour 等自动发现（地址变了就重新扫码）、iPhone 存多台 Host（扫新码即替换）、出局域网访问（VPN、Tailscale 等，监听网卡与子网的规则不变）、双向 TLS、iPhone 端「忘记 Host」。
- 回环地址（显式 `--listen 127.x`，开发、真实后端 E2E、模拟器联调用）照旧明文、不鉴权。桌面的「局域网访问」从不监听回环。

## Host 端

### 证书

局域网服务第一次启动时生成自签证书（rcgen，ECDSA P-256，有效期 10 年），存在设置目录的 `tls/` 下：桌面是 `~/Library/Application Support/dev.joy.codeck/tls/`，daemon 在状态目录里。不放缓存目录（会被清）。私钥文件 0600。iPhone 固定的是证书 DER 的 SHA-256；不删这个目录，证书就不变。

局域网监听（非回环）改用 rustls 提供 HTTPS 与 WSS；TLS 握手失败只断这条连接，不影响监听。

### 设备表

`devices.json`（与证书同目录）每台设备一条：`{id, name, pairedAt, lastSeenAt, tokenSha256}`。token 是 32 字节随机数（base64url），只在配对应答里出现一次，Host 只存它的 SHA-256。`lastSeenAt` 只在这台设备的 WebSocket 连上时更新。设备名最长 64 个字符，超出截断。

### 配对

- 「配对新设备」生成 16 字节随机配对密钥，只在内存里，5 分钟后或用过一次即作废；同一时刻只有一个有效，再生成一个就作废上一个；关掉配对弹层（`lan_pair_cancel`）也作废。
- 配对链接（即二维码的内容）：

  ```text
  codeck://pair?h=192.168.2.165,10.99.0.1&p=38471&fp=<证书 SHA-256，base64url>&k=<配对密钥，base64url>
  ```

  `h` 是此刻所有局域网监听地址（不含回环），按监听顺序。
- `POST /v1/pair`，body `{key, name}`：密钥有效则新增一台设备，返回 `{deviceId, token}`，并发 `lan-devices-event`（载荷是设备列表，不转发给局域网客户端）；密钥不对、过期或已用过一律返回 `code: "pairing_expired"`。
- 二维码由 Rust 端生成（`qrcode` crate）：桌面拿 SVG，daemon 在终端里用 Unicode 方块打印，前端不加依赖。

### 鉴权

- 不要求 token：`GET /health`（返回 `product` 与 `apiVersion`）与 `POST /v1/pair`。
- 其余（`/v1/rpc`、`/v1/events`、`/web/*`）要求 `Authorization: Bearer <token>`，按 SHA-256 查设备表；查不到返回 401 与 `{error: {code: "unauthorized", message: "这台设备没有配对或已被移除，请重新扫码配对。"}}`。
- 每条 WebSocket 记下所属设备。移除设备时立即关掉它的 WebSocket，之后的请求都是 401。
- 来源子网、Host 头、Origin 的校验照旧，配对接口同样受它们约束。
- `apiVersion` 1 → 2。旧版 App 连不上新 Host，按版本不一致或连不上处理。

### 管理入口

- 桌面：「设置 › 通用 › 局域网访问」打开时多两样：「配对新设备」（弹层：二维码、链接、倒计时；收到 `lan-devices-event` 里新增的设备即写「已配对 <设备名>」并关闭）与「已配对的设备」（名字、配对时间、最近连接、「移除」需确认）。命令表加四个 `local` 命令：`lan_pair_start() -> PairingOffer {url, svg, expiresAt}`（局域网访问没开或没有地址时报错）、`lan_pair_cancel()`、`lan_devices() -> Vec<PairedDevice>`、`lan_remove_device(id)`；mock 同步实现。开关下的说明改为讲配对，不再写「没有鉴权」。
- daemon：状态目录里的 `control.sock`（unix socket，0600），每行一个 JSON 请求与应答。`codeck-host pair` 打印二维码、链接与「5 分钟内有效」，等到配上或过期再打印结果；`codeck-host devices` 列出设备；`codeck-host unpair <id>` 移除。daemon 没在运行时报「codeck-host 没有在运行（<状态目录>）」。用 socket 而不是交换文件，是因为移除设备要由运行中的进程立即断开那台设备的连接。

## iPhone 端

### 入口

- 「连接 Host」页改为扫码页：App 内相机取景（Info.plist 加 `NSCameraUsageDescription`），扫到配对链接即开始配对；已有 Host 时说明会替换它。
- App 注册 `codeck` 链接。从系统相机或别处打开 `codeck://pair?…` 时先确认「与 Host 192.168.2.165 配对？会替换当前的 Host。」，确认后才配对，免得网页上的链接把手机配到别人的 Host 上。地址规则同现在（只收私网 IPv4），格式不对的链接报错。
- DEBUG 版保留手输 `127.0.0.1`，连明文的回环 daemon。模拟器上测 TLS 配对用 `xcrun simctl openurl` 喂链接。

### 配对过程

1. 按 `h` 的顺序请求 `/health`（TLS，只认 `fp`，每个 3 秒超时），用第一个有应答的地址。
2. 已存的指纹等于 `fp`、已存的 token 在这个地址上仍有效时（地址变了重扫的情形），只更新地址，不用配对密钥，不新增设备。
3. 否则调 `/v1/pair`，`name` 取 `UIDevice.current.name`。
4. 成功后地址、端口、指纹、设备 ID 存 UserDefaults，token 存钥匙串（`kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`，不随 iCloud 同步）。整个 App 只存一台 Host。

### 连接

新的 URLSession 代理只认 DER 的 SHA-256 等于已存指纹的证书，其余一律取消。`HostClient`、`EventStream` 与下面的 Transcript 页中转都用它，请求与 WebSocket 都带 `Authorization`。重连、前后台、`hello` 后整体重新同步的逻辑不变。

### 出错

| 情况 | 文案 |
|---|---|
| 401 | 这台 iPhone 已被 Host 移除，请重新扫码配对。 |
| 证书指纹对不上 | Host 的证书与配对时不同，请重新扫码配对。 |
| 连不上 | 连不上 Host（192.168.2.165:38471）。（同现在） |
| `pairing_expired` | 配对码已过期或已用过，请在 Host 上重新生成。 |
| 二维码里的地址都连不上 | 连不上二维码里的 Host。 |

离线横幅在「重试」旁加「重新扫码」。

### 升级

旧版存的明文地址视为没配对，打开即进扫码页，旧的 UserDefaults 键删掉。

## Transcript 页经 App 中转

WKWebView 对页面自己发起的 WebSocket 不能放行自签证书（Apple DTS 在论坛确认，rdar 25491679），子资源请求是否走 `webView(_:didReceive:)` 也没有可靠说法。所以页面不直接连 Host：

- **静态文件**：页面从 `codeck-web://host/web/transcript.html?…` 加载，`WKURLSchemeHandler` 用固定指纹的会话带 token 去取 `https://<Host>/web/…`，状态码、`Content-Type`、CSP 原样转回；只处理 `/web/` 下的 GET。
- **RPC**：页面 `window.webkit.messageHandlers.codeckRpc.postMessage({method, args})`（`WKScriptMessageHandlerWithReply`），App 转发到 `/v1/rpc`，原样回 `{result}` 或 `{error}`；网络出错回 `{error: {code: "http", message: "连不上 Host"}}`。
- **事件**：App 的 `EventStream` 把每条事件原文（含 `hello`）经 `callAsyncJavaScript` 交给 `window.codeckPage.event(e)`，页面不再开自己的 WebSocket；App 重连后的 `hello` 照常触发页面的 `onResync`。
- **前端**：`remote.ts` 拆出传输层——HTTP 传输即现在的 fetch 加 WebSocket（回环开发、真实后端 E2E、浏览器），App 中转传输在有 `messageHandlers.codeckRpc` 时由 `ipc/index.ts` 选用。方法表不动，`remote.test.ts` 照旧；复制文字与 `setConnect` 的桥不动。

token 不进网页（没有 Cookie），WKWebView 接触不到证书，每台 iPhone 只剩一条 WebSocket。

## 文档

- 新 ADR-0017「局域网访问改用 TLS 加扫码配对」：威胁模型、上面的取舍、Transcript 页改走 App 中转的原因；修订 ADR-0011、ADR-0012 里「不鉴权」的几条后果。iPhone 的 `codeck://pair` 会替换 Host，与 ADR-0014（桌面的链接只打开）的关系写明：链接只提出配对，要人确认。
- spec 的「局域网访问」与 iPhone 部分、README「iPhone 与 Host」（安全说明重写，加配对步骤）、CONTEXT（加「配对」）、codeck-host / ios / web 的 CLAUDE.md 同步。
- 设计画布新建「方案 · 配对」页：桌面配对弹层与设备列表、iPhone 扫码页与确认框，各出 2–3 个可编辑画板，选定后再实现界面。

## 测试

- Rust：配对密钥只能用一次、5 分钟过期、再生成即作废上一个；token 校验与移除设备后立即断开；局域网监听上不带 token 是 401、回环照旧放行；rustls 客户端固定指纹走完配对 → RPC → WebSocket 的端到端用例；证书与设备表重启后还在；`control.sock` 的 pair / devices / unpair。
- iOS：配对链接解析（多个地址、格式错误、非私网地址）、指纹校验、错误到文案的映射、同指纹且 token 有效时只更新地址、旧版地址升级后进扫码页。
- 前端：App 中转传输的选用与收发（vitest，假的 `messageHandlers`）；桌面配对界面的 mock E2E（二维码与倒计时、收到事件后显示配好、移除设备）；真实后端 E2E（回环）照旧通过。

## 实测

- 模拟器：daemon 监听 en0，另用端口与独立的状态目录；`codeck-host pair` 的链接经 `simctl openurl` 喂给 App，看 Session 列表、Transcript 与代码高亮，Connect 一个沙箱 Session；`unpair` 后看 401 文案，换掉证书后看指纹文案。
- `curl`：明文 `http://<IP>:<端口>/health` 不通；`curl -k` 不带 token 请求 `/v1/rpc` 是 401。
- 桌面：隔离构建的实机打开「局域网访问」，iPhone 18 Pro Max 真机扫码配对，再移除。

## 实施顺序

每步一个独立提交：

1. 小验证（模拟器）：WKWebView 从自定义 scheme 加载页面、Shiki 的 Worker 与 wasm，`URLSessionWebSocketTask` 经代理固定自签证书。Worker 不行时先改成内联 blob Worker；还不行就只让静态文件走顶层 HTTPS 导航配合 `webView(_:didReceive:)` 放行，RPC 与事件仍走 App 中转。
2. Host：TLS、设备表、配对、鉴权、`control.sock` 与命令行。axum 上接 rustls 的写法按所用版本的文档定。
3. 前端：传输层拆分、桌面配对界面（画布选定后）、mock 与 E2E。
4. iOS：固定指纹的会话、扫码页、链接入口、Transcript 页中转、错误文案。
5. ADR、spec、README、CONTEXT、CLAUDE.md 收尾。
