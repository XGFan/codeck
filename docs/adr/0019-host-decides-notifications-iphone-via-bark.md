# 通知改由 Host 判断，iPhone 经 Bark 收推送

桌面的系统通知（等待作答、已完成）原先全在桌面前端判断（`web/src/features/attention/`）：「看过」只认桌面窗口，延迟存在 localStorage，只跑 daemon 时不发。iPhone App 退到后台就断开与 Host 的连接，iOS 也不让 App 在后台常驻连接，所以 App 自己发不了「人不在时」的通知，得有推送服务把通知送到手机。

- **判断挪进 Host**（`crates/codeck-host/src/attention/`）：Host 已经知道每个 Live Session 的状态、每个连接的请求卡片和缓存保活，各端再告诉它「正在看哪个 Session」（`set_viewing`），由它决定发不发、发到哪一端、何时删。桌面与 daemon 同一套规则。
- **Mac 通知**经桌面外壳注入的 `Notifier` 发出（沿用 `notify.rs` 的 `UNUserNotificationCenter`），它同时回答「Mac 此刻有没有人在用」（最近 2 分钟有键盘或鼠标操作，且没锁屏）。daemon 没有 `Notifier`。
- **iPhone 经 Bark**（第三方的推送 App，带自己的 APNs 证书）：Host 向 Bark 服务器（默认 `https://api.day.app`，可换自建的）发一个 HTTP 请求，通知带 `url=codeck://session/<id>`，点开由 Bark 交给系统、打开 CoDeck 的这个 Session。同一 Session 的推送以 `id` 替换，处理完以 `delete` 删掉。
- **发到哪一端**：到点时 Mac 有人在用（或没配 Bark）就发 Mac，否则推 iPhone；发到 Mac 还没处理的，人离开 Mac 或满「补推」时长后再推 iPhone。任一端看过或作答，两端的通知都删掉。

## Considered Options

- **APNs，Host 直接推**：通知是 CoDeck 自己的，不经第三方 App，也不用自建服务器（Host 用 .p8 key 签 JWT 调 Apple 的接口）。但要自己的付费开发者账号：现在装机用的是第三方的 Ad Hoc 描述文件，带 `aps-environment`，推送的 key 却在别人的账号里。有了自己的账号再换，Host 侧的判断不变，只换发送这一层。
- **Local Push Connectivity（`NEAppPushProvider`）**：系统在指定的 Wi-Fi 上替 App 连着 Host，不出局域网。要向 Apple 申请 `app-push-provider` entitlement（也得是自己的账号），现在的描述文件里没有；离开家里的 Wi-Fi 就收不到。
- **只在 App 开着时发本地通知**：不需要任何服务，但人不在手机前时正是收不到的时候。
- **判断留在桌面前端，只加一个发 Bark 的命令**：改动小，但 daemon 推不了；窗口隐藏后 WebKit 会放慢计时器，补推的时刻不准；iPhone 的「正在看」还得经 Host 转给前端。

## Consequences

- 推 iPhone 要 Host 能连外网，iPhone 上装 Bark（`id` 替换与删除要 Bark 1.5.2、bark-server 2.2.5 以上）；删除走 Bark 的静默推送，要给 Bark 打开「后台 App 刷新」，没开时手机上留着旧通知，点开只是打开那个 Session。
- 通知的设置从 localStorage 搬进 `settings.json` 的 `notify`，首次启动时迁移；daemon 改它状态目录里的这个文件。
- 只跑 daemon 时也能推 iPhone；开发版（`pnpm tauri dev`）没有 Mac 通知，配了 Bark 就一律推 iPhone。
- 「已完成」的状态也在 Host，桌面侧栏读 Host 的事件；iPhone 暂不显示。
- iPhone 除 `codeck://pair` 外也认 `codeck://session/<id>`，格式与桌面相同（ADR-0014）：只导航，不用确认。
