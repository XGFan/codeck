# 订阅额度经无界面的 claude 读取

订阅账号的额度（当前 5 小时、本周全部模型、按模型的周额度，各自的百分比与重置时刻）只有 Claude Code 自己知道，本机的 transcript 里没有。Host 需要时起一个无界面的 claude（`claude -p --input-format stream-json --output-format stream-json --no-session-persistence`，不加载 MCP 与 hooks），在它的控制通道上发一条 `get_usage` 请求，读到应答就关掉。不发提示词，约 1 秒，不产生费用。结果在 Host 里缓存 60 秒，同时只跑一个。命令 `get_plan_usage` 只给桌面（`local`），daemon 与 iPhone 不提供。

额度是登录环境里那个 claude 所登录账号的，与 CoDeck 的数据根设置无关。读不到订阅额度的账号（API key 登录等，应答里 `rate_limits_available` 为 false）返回空，界面整块不显示。

## Considered Options

- **接管 statusLine**：statusLine 的 JSON 里有 `rate_limits`，文档写明了；但要改用户的 `statusLine` 设置（常已配着别的状态栏），还得有一个 Session 在跑才会有数据。
- **经 ccrpc 在已有的 Session 里发 `/usage` 再读屏**：没有 Session 时读不到，而且会打断那个 Session 的界面。
- **直接调 claude.ai 的接口**：要自己拿登录凭据，CoDeck 不碰凭据。
- **由本机记录推算**：推不出百分比（不同模型折算不同），窗口的起点也推不准（别的设备上的用量本机看不到）。

## Consequences

- `get_usage` 是实验性的控制请求，没有文档（claude 2.1.331 实测）。解析一律宽容：只取认识的字段，不认识的额度种类跳过；应答变了读不出时报错，界面只影响额度这一块，统计其余部分照常。
- 这是 Host 第一处直接起 `claude -p`。ccrpc 的 ADR-0003「不用 `-p` 驱动 Session」不受影响：这里不驱动任何 Session，只读账号状态。
- 对数据根只读（ADR-0002）不变：CoDeck 自己不写；这个 claude 进程会像任何 claude 一样在它的配置目录里写自己的进程文件（Connect 新建、Resume 的 Session 也是 CoDeck 起的 claude 在写，ADR-0007）。它的工作目录是 Host 专用的 `<缓存目录>/plan-usage`，Engine 读进程文件时丢掉工作目录等于它的那些，所以不会闪现成 Live Session；别人用 `claude -p` 跑的 Session 照旧。
- 「本周期」跟随订阅：周期的边界取额度里每周重置的时刻，往前每 168 小时一段；订阅的重置时刻变了，历史周期的边界跟着变。锚存在本机：还没读到、或读取失败时沿用上次存的；只有确知账号没有订阅额度时才清掉它，改用设置里手填的重置时间。
- 缓存过期后的每次读取起一个进程（约 1 秒）。桌面窗口在前台时每 5 分钟读一次，回到前台时过期则重读，手动刷新不走缓存。
