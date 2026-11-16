# 认不出的界面在独立的终端窗口里处理，直连 Holder

ccrpc 认不出的界面（`/mcp` 这类面板、没见过的对话框）以前只能在卡片上看屏幕原文、按 Esc 关掉，别的操作要回到 ToH 客户端的终端里做。我们在这张卡片上加「在终端窗口中处理」（「···」菜单另有「在终端窗口中打开」，Session 在 clay 里就有，不需要 Connect）：打开一个独立的 tauri 窗口，里面只有这个 Pane 的终端，xterm.js 经插件 `crates/codeck-term` 直连 Holder 的 unix socket（toh 帧协议），和 ToH 客户端看到的是同一个画面，键入直接写进 PTY。

这是过渡做法：ccrpc 认得的界面越来越多，这个窗口用得会越来越少，所以它整块独立，删掉时不碰其他功能——插件 crate（命令与权限自带）、两处 capability、单独的页面 `terminal.html` 与 `features/terminal`、主窗口里一个同步 hook、卡片上的一个按钮与「···」菜单的一项。它不进 `Api` 与 bindings，也不进 Host 的命令表：窗口与 Channel 只有桌面有，daemon 与 iPhone 不提供（iPhone 上有 ToH 自己的 App）。

尺寸照 ccrpc 的 ADR-0001：PTY 尺寸归人的 ToH 客户端，ccrpc 读不懂屏幕时才调成 120×40。终端窗口带 `size_events` 附着、跟着 Holder 报来的尺寸走，自己从不发 `Resize`；窗口按 Pane 的列×行、13px 打开，之后字号随窗口缩放。这样窗口出现或拖动都不会让 Claude Code 重画、不打乱 ccrpc 的识别。

## Considered Options

- **卡片里直接换成活的终端 / 会话区下半部分的终端面板 / 会话区切到终端视图**（设计稿方案 A、B、C）：宽高受会话区限制，120 列在窄窗口里字太小；面板与视图还要管开关和高度这些状态，过渡用不值得。
- **在 PTY 里跑 `toh attach`**：终端套终端，`Ctrl-\` 前缀键冲突，而且 `toh attach` 会抢尺寸。
- **iframe 内嵌 ToH 的 web 页面**：要开 `[web]`、配对与 HTTPS，界面里还多一层 ToH 自己的标签栏。
- **经 ccrpc 转发按键**：ccrpc 只会发它认得的命令；做成通用的按键与屏幕转发等于在 ccrpc 里再造一个终端客户端，ccrpc 自己连 Holder 也不受影响，直连更简单。

## Consequences

- xterm.js 6 + Unicode 11 宽度表：与 Holder 的 VT（alacritty_terminal）逐格一致（原型实测 0 处不同），中文输入法可用。
- 终端的应答（光标位置、设备属性等查询）由 Holder 从非宿主客户端的输入里滤掉，不会变成键入。
- Claude Code 开着鼠标上报，滚轮与点击都交给它；选中文字复制要按住 ⌥ 拖动。
- 窗口关闭只看 Pane：Pane 里不再有 Session（不在 `ccrpc ls` 里、也没有连着的连接）时由主窗口关掉；Disconnect、ccrpc 退出、卡片消失（Claude Code 回到输入框）都不关，后者只在底栏提示；主窗口没了时一起关。主窗口重新载入后，已开的终端窗口要等 `ccrpc ls` 与连接回来才重新受管。
- 「···」菜单的「隐藏 Pane / 显示 Pane」不变（那是 Pane 在 ToH Mac 客户端里的摆放，成对使用），终端窗口是旁边另加的一项。
- 找 Holder 用 `toh ls --json`（登录 shell 的环境，与 ccrpc 一样不读 ToH 的状态目录），经 Host 的 spawn 锁、10 秒超时。
