# 内容只来自 ccrpc

cc-gui 显示的一切（消息、状态、请求、后台任务）都只经 ccrpc 的协议得到，不像 claude-viewer 那样直接读 transcript、进程文件，也不依赖 cv-core。直接读能更快拿到更全的内容，渲染也能整段复用 claude-viewer，但 cc-gui 的目的是在日常使用里验证 ccrpc：GUI 一旦绕过 ccrpc 去读文件，ccrpc 的缺口就被它悄悄补上，验证失去意义。所以 ccrpc 给不了的东西，要么补进 ccrpc，要么在 v0 里缺着、回 TUI 处理。

## Consequences

- 渲染组件从 claude-viewer 复制后改成吃 ccrpc 的 block，不共享 cv-core 的数据模型。
- GUI 发现缺少的信息时，改动落在 ccrpc（协议与 spec），而不是 GUI 自己去取。
