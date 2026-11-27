# 局域网访问改用 TLS 加扫码配对

ADR-0011、ADR-0012 让 iPhone 经局域网连 Host，暂不鉴权：同一网段的任何设备都能读 Session、发提示词、新建 Session，等于能在 Host 上执行命令；咖啡馆、酒店的 Wi-Fi 也是私网地址，来源子网校验挡不住，明文 HTTP 还能被抓包。现在局域网上一律 TLS，每台设备凭自己的 token 访问，首次配对扫码。

- **证书**：Host 自签一张证书（ECDSA P-256，10 年），存在 settings.json 旁的 `tls/` 里，不随重启变。iPhone 固定的是证书 DER 的 SHA-256，不走系统的信任链。
- **配对**：桌面在「局域网访问」里点「配对新设备」、daemon 运行 `codeck-host pair`，生成一个 5 分钟内只能用一次的配对密钥（只在内存里，单调钟计时，兑换成功、写盘之后才作废）。二维码是 `codeck://pair?h=<地址>&p=<端口>&fp=<指纹>&k=<密钥>`：指纹随码带过去，iPhone 不用信任首次连接，配对时也不怕被中间人截获。`POST /v1/pair` 用密钥换回这台设备的 token（32 字节随机数，Host 只存它的 SHA-256）。
- **鉴权**：除 `/health` 与 `/v1/pair` 外都要 `Authorization: Bearer`。每台设备一份 token，可以在 Host 上单独移除，移除时立即断开它的连接。子网、Host 头、Origin 的校验照旧。
- **找 Host**：iPhone 只存一台 Host、只记地址；地址变了就重新扫码，证书没变且 token 还有效时只更新地址，不新增设备。
- **回环**：显式 `--listen 127.x` 照旧明文、不鉴权，留给开发、真实后端 E2E 与模拟器联调，只有本机连得到。桌面的「局域网访问」从不监听回环。
- **Transcript 页经 App 中转**：WKWebView 对页面自己发起的 WebSocket 不能放行自签证书（Apple DTS 确认，rdar 25491679），所以页面不直接连 Host：App 用自定义 scheme（`WKURLSchemeHandler`）代取 `/web/*`，RPC 走带应答的消息处理器，事件由 App 的 WebSocket 推给页面。token 不进网页，WKWebView 接触不到证书。

## Considered Options

- **只加 token、不加密**：最轻，但公共 Wi-Fi 上 token 会被看到并重放，等于没有鉴权。
- **Bonjour 列出 Host 再输配对码**：不用摄像头，但短码要防配对时被中间人截获，得加 PAKE（如 SPAKE2）或让人核对指纹。
- **Bonjour 自动找回变了的地址**：daemon 要内置 mDNS，App 多一条 NSBonjourServices；地址常变的网络里才有好处，这次先靠重扫。
- **双向 TLS（每台设备一张客户端证书）**：不传 token，但 WKWebView、`URLSessionWebSocketTask`、rustls 三处都要处理客户端证书，在这个威胁模型下没有多防什么。
- **页面带 Cookie 直连 Host**：前端不用改，但 WebSocket 过不了自签证书，见上。
- **常显二维码、密钥长期有效**：什么时候扫都行，但屏幕被拍下或截图后谁都能来配。

## Consequences

- 修订 ADR-0011、ADR-0012 里「不鉴权，只在可信网络里用」的后果：打开局域网访问不再等于向整个网段开放。仍只监听局域网网卡的私网地址（不含 VPN、Tailscale），出局域网访问不在这次范围。
- `apiVersion` 升到 2，旧版 App 连不上新 Host。
- 删了 `tls/` 目录 Host 就换了证书，所有设备要重新扫码（iPhone 报「证书与配对时不同」）；删了 `devices.json` 则所有 token 作废。
- iPhone 注册 `codeck` 链接，但只认 `codeck://pair`：链接会替换 Host，所以从系统相机或别处打开时要人确认（与 ADR-0014 桌面的链接只导航、不写的原则一致：链接只提出配对）。
- daemon 的 `pair` / `devices` / `unpair` 经状态目录里的 `control.sock`（0600）和运行中的进程交互：移除设备要由它立即断开那台设备的连接。
- 页面在 App 里不再开 WebSocket，每台 iPhone 只有一条连接；浏览器与真实后端 E2E 经回环照旧走 HTTP 与 WebSocket。
