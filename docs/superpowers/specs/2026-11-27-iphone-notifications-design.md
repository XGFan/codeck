# iPhone 通知与 Mac / iPhone 联动

桌面的系统通知（等待作答、已完成）原先全在桌面前端判断，「看过」只认桌面窗口，只跑 daemon 时不发；iPhone 没有通知。这次把判断挪进 Host，iPhone 经 Bark 收推送、点开进 CoDeck 的 Session，两端互相知道对方看过、处理过什么（ADR-0019）。

## 目标与边界

- 判断在 Host：桌面与 daemon 同一套规则；任一端看过或作答，两端的通知都删掉。
- 到点时 Mac 有人在用就发 Mac，否则推 iPhone；发到 Mac 还没处理的，人离开 Mac 或满「补推」时长后再推 iPhone。
- iPhone 经 Bark 收推送，点开跳进 CoDeck 的那个 Session。
- 不做：APNs 与 Local Push（要自己的开发者账号，见 ADR-0019）；iPhone 上的「已完成」标记、App 内的通知与通知设置；推送失败重试。

## Host 端

新模块 `crates/codeck-host/src/attention/`：`rules.rs` 是纯函数，`mod.rs` 每秒推进一次、收集输入、执行动作。

### 输入

- **Live 状态**：worker 的 `LiveChanged`（进程文件），busy / waiting / idle；状态未知的（agy 的 Session）不参与，同现在。
- **已连接的**：ConnState 的 `state`（busy / idle / waiting）优先于进程文件；有请求卡片的算 waiting，与 Dock 角标同一口径。
- **缓存保活**：Host 正在给某个 Session 续缓存（`keepalive.rs`，`/btw` 面板开着的几秒）时，它保持原来的状态，不算等待作答，「已完成」也不清。
- **正在看**：`set_viewing`，见下。
- **Mac 有人在用**：`Notifier::present()`，见下；没有 `Notifier` 时当作没人。

### 规则（`rules.rs`）

从 `web/src/features/attention/rules.ts` 移植，原有的不变：

- 每个 Live Session 记着状态、从何时起、看过没有、这个状态的通知发过没有；一轮结束（busy → idle）时没人在看就标「已完成」，看了、又工作或等待作答、不再 Live 时清掉。
- 等待作答或已完成持续到设定的时长、期间没人看过，到点；同一时刻到点的合成一条。标题、正文、`id`（单个 `session-<id>`，合成的 `sessions-<时间>`）照旧。
- 启动时已在等待作答的从 Host 启动时算起；上次运行发的通知不管。

新增的：

- **看过**：任一端正在看它就算看过（`set_viewing` 记着的所有调用方）。
- **发到哪**：到点时有 `Notifier` 且（Mac 有人在用，或没配 Bark）→ 发 Mac；否则配了 Bark → 推 iPhone；都没有 → 不发（仍记为发过，不攒着）。
- **补推**：一条只发到了 Mac、还没处理完的通知，配了 Bark 且下面任一条件成立时推 iPhone，每条只补一次：Mac 转为没人在用；发出后满「补推到 iPhone」的时长（设为关闭时只看前一条）。补推的内容按当时重算（「等待作答 8 分钟」），合成的那条只算其中还没处理完的 Session，只剩一个时按单个的写；`id` 沿用这条通知的，处理完时两端按同一个 `id` 删。
- **处理完**：一条通知里的 Session 都看过了、状态变了或不再 Live 时，从发到过的每一端删掉（Mac 按 `id` 删，Bark 发 `delete`），沿用 `sweep`。
- 输出是一串动作：发 Mac、删 Mac、推 iPhone、删 iPhone，由 `mod.rs` 执行。每条发出的通知记着 `{id, sessions, at, mac: 发出时间?, phone: 发出时间?}`。

### Notifier（Mac 通知）

```rust
pub trait Notifier: Send + Sync {
    /// 最近 2 分钟有键盘或鼠标操作，且没锁屏。
    fn present(&self) -> bool;
    fn send(&self, notice: Notice);
    fn remove(&self, ids: Vec<String>);
}
```

- desktop 实现：发与删沿用 `desktop/src/notify.rs`；闲置用 `CGEventSourceSecondsSinceLastEventType`（合并的会话状态、任意输入），锁屏读 `CGSessionCopyCurrentDictionary` 的 `CGSSessionScreenIsLocked`；读不出时当作有人在用。2 分钟写死，不做成设置。
- 只有 app bundle 能用通知中心：`pnpm tauri dev` 下 desktop 不注入 `Notifier`，配了 Bark 就一律推 iPhone。daemon 没有 `Notifier`。
- 点通知照旧：外壳把窗口带到前面、发 `notification-click-event`，前端走 `codeck://` 链接同一条路。
- 前端不再发通知：`Api.notify` / `removeNotifications` 与 desktop 的这两个命令删掉，`onNotificationClick` 保留。

### Bark（`attention/bark.rs`）

- 推送：`POST <服务器>/push`，JSON `{device_key, title, body, url: "codeck://session/<id>", group: "CoDeck", id, isArchive: "0"}`，含等待作答的加 `level: "timeSensitive"`；合成的那条 `url` 指向第一个等待作答的 Session（没有则第一个已完成的），同 Mac。
- 删除：`{device_key, id, delete: "1"}`。
- 响应的 HTTP 状态或 JSON 的 `code` 不是 200 算失败，带上它的 `message`。
- 用现有的 `ureq`，超时 10 秒，放在 `spawn_blocking` 里，不卡每秒的推进。

### 正在看（`set_viewing`）

- 新的 `remote` 方法 `set_viewing(session_id: Option<String>)`，Host 按调用方记：桌面窗口（进程内）一份，局域网按设备 id（来自 token），回环（开发、真实后端 E2E）一份。
- 局域网设备的事件连接全断了（iPhone 退到后台就断）就清掉它的记录；桌面窗口失焦、隐藏时由前端报 null。
- 记录一变就立即推进一次规则，不等下一秒。

### 事件

- `attention-event { done: string[] }`：「已完成」的 Session，变了才发；桌面侧栏用。不转发给局域网（iPhone 暂不用）。
- 原来的 `keep-alive-event` 不变（前端的 Dock 角标、「别处在等」还用它）。

### 设置

`settings.json` 的 `notify`：

```json
"notify": {
  "waitingMinutes": 3,
  "doneMinutes": 3,
  "escalateMinutes": 5,
  "bark": { "server": "https://api.day.app", "deviceKey": "…" }
}
```

- 数字是分钟（等待作答、已完成的 0 是立即），`"off"` 是关闭，不写用默认值：等待作答、已完成各 3 分钟，补推 5 分钟。
- `bark.deviceKey` 为空或不写就是没配 Bark；`server` 不写用 `https://api.day.app`，去掉末尾的 `/`。
- `local` 命令：`notify_settings` / `set_notify_settings`（照 `keep_alive_minutes` 的做法），`test_bark(server, device_key)` 用框里当前的值发一条「CoDeck · 测试推送」（不保存），返回成功或失败原因。
- 迁移：前端启动时 localStorage 有 `codeck.notify`、而 `notify` 里还没有这两项时，写进去，再删掉 localStorage 的这一项。

### 出错

- 推 iPhone 失败（网络不通、返回码不对）：写 Host 日志，不重试，这条不再补推；记下「最近一次推送失败」（原因与时间），设置里显示，下次推送成功后去掉。
- 删除失败：只写日志。
- Bark 没开「后台 App 刷新」时删除不生效，手机上留着旧通知；点开只是打开那个 Session，接受。

## 桌面前端

- `features/attention/`：删掉 `rules.ts`、`rules.test.ts` 与 store 里的 `tracks` / `sent` / `delays`；`keeping` 留着（`useWaiting` 用）。`useAttention` 只剩两件事：窗口在前台时把当前 Session 报给 `set_viewing`（焦点、可见性、切换 Session 时；失焦、隐藏报 null），收 `attention-event` 存「已完成」，`useDone` 读它。
- 设置 › 通用：「等待作答通知」「已完成通知」改为读写 Host 的设置，选项不变；新增「补推到 iPhone」（关闭 / 1 / 3 / 5 / 10 分钟，默认 5，说明：Mac 有人在用时先发 Mac，离开 Mac 或满这个时长还没处理就推到 iPhone）；新增「iPhone 推送（Bark）」：服务器、Device Key、「保存」、「发送测试推送」（显示「已发送」或原因），下面一行「要在 iPhone 的设置里给 Bark 打开「后台 App 刷新」，处理完的通知才会从手机上删掉。」，有最近一次失败时再一行「最近一次推送失败：<原因> · <时间>」。
- mock：`set_viewing` 记下调用（E2E 探针可读）、通知设置的读写、`test_bark`、按 mock 的场景发 `attention-event`。

## iPhone 端

- `onOpenURL` 认 `codeck://session/<id>`（id 是 UUID，大小写不限，参数忽略，同 ADR-0014）：已配对时打开这个 Session 的页面，列表还没加载完时加载完再打开，Host 上找不到时提示「找不到这个 Session。」；没配对时忽略。`codeck://pair` 照旧。
- Session 页出现且 App 在前台时调 `set_viewing(id)`，离开页面时报 null；退到后台连接断开，Host 自己清掉。
- 不申请通知权限：通知由 Bark 显示。

## 测试

- Rust 单测（`rules.rs`，重点）：移植 `rules.test.ts` 的用例；发到哪（有人在用 → Mac；没人 → Bark；没配 Bark → Mac；没有 Notifier 也没配 Bark → 不发）；补推（离开触发、超时触发、超时关闭只看离开、每条只补一次、内容重算、合成的只算未处理的）；跨端看过；两端删除；缓存保活期间不算等待作答。
- Rust：Bark 请求体的组装（推送与删除）；设备的事件连接断开后清掉它的「正在看」；`notify` 设置的解析（数字、`"off"`、不写）。
- 前端：vitest 覆盖迁移；E2E（mock）覆盖设置项的读写、「发送测试推送」的两种结果、侧栏的「已完成」来自 `attention-event`、窗口里打开 Session 时调 `set_viewing`；现有通知相关的用例同步改。
- iOS：`codeck://session` 的解析与打开（已配对、未配对、列表未加载、找不到）。
- 实测（打包版桌面应用，延迟与闲置判定临时调到约 10 秒，推到 iPhone 18 Pro Max，Device Key 由你在设置里填）：Mac 在用时只发 Mac；闲置时只推手机；超时补推与离开补推；在手机上看过后 Mac 的通知被删；在 Mac 上作答后手机的通知被删；点 Bark 的通知进 CoDeck 的那个 Session（冷启动与已在后台各一次）；只跑 daemon、配了 Bark 时能推。

## 文档

- ADR-0019。
- spec.md：重写「系统通知」（Host 判断、发到哪、补推、两端删除、Bark 的设置），「已完成」改为状态在 Host；iPhone 的链接。
- README：Bark 的配置步骤。
- CLAUDE.md：`web/CLAUDE.md` 里「已完成」与系统通知那段改写，`crates/codeck-host/CLAUDE.md` 加 `attention/`，`ios/CLAUDE.md` 加 `codeck://session` 与 `set_viewing`，`desktop/CLAUDE.md` 写 `Notifier`。
- 设计画布：设置页若画在某个区域页里，原型与便签同步。

## 实现顺序

1. Host：`attention/`（规则与单测、Bark、设置、`set_viewing`、事件），desktop 的 `Notifier`。
2. `pnpm bindings`，前端改造、mock、E2E。
3. iOS：链接与「正在看」。
4. 实测，更新文档。
