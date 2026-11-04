# 受控 Session 必须装 ccrpc 插件，插件不改变 Claude Code 的表现

ccrpc 原先对 Claude Code 完全透明，审批、AskUserQuestion、轮次边界都靠屏幕识别与 transcript 推断，脆弱且要靠启发式（屏幕与工具调用的匹配、等 3 秒、`turn_duration` 加超时）。现在改为：Claude Code 必须装 ccrpc 插件才能成为受控 Session。插件只含 hooks，只在 clay Pane 里生效，hook 不向 stdout 写任何会进上下文的内容；审批与 AskUserQuestion 经 PermissionRequest hook 作答，轮次、Session 切换、Subagent 起止以 hook 为准。底线是**不改变 Claude Code 的表现**：人在 TUI 上看到的界面与能做的操作，与没有插件时一样，所以只有阻塞时界面照常显示的 hook 才能扣住等 ccrpc。

## Considered Options

- **没有插件时回落到屏幕路径**：调用方无感，但两套并存，代码一行不少，还要测两套。
- **hook 只报告、作答仍按键**：最保守，但审批框识别、按键这一套全要保留，收益只在信号上。
- **插件也允许调用方在 PreToolUse 拦截任意工具调用**：能力更大，但每次工具调用都要往返，会盖过人自己的权限规则。

## Consequences

- 插件是否加载无法从文件判断（运行中装的要 `/reload-plugins`）：以插件的 SessionStart hook 在 ccrpc 状态目录的报到为准，没有报到的 Session 不能 attach，需重启 claude。
- 后台 Subagent 的审批在 hook 阻塞期间不显示审批框，因此不能扣住：hook 只报告，ccrpc 仍按屏幕上的审批框按键作答（布局库与分析器照旧覆盖审批类布局）。ExitPlanMode 的“同意”hook 给不了，计划审批仍是按屏幕作答的 choice。
- hook 作答时 TUI 多一行 `Allowed / Denied by PermissionRequest hook`，transcript 多一条 `hook_permission_decision`；每次 Stop hook 写一条 `stop_hook_summary`。这些不进模型上下文。
- 一轮改为与 Claude Code 一致：一条提示词一轮，后台任务的结果通知自成一轮。协议升为 2。
- 写入与读屏幕仍只经 ToH：提示词、按键、其余界面不受影响。
