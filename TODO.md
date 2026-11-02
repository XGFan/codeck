# TODO

- 人与调用方同时写入的冲突处理（接手语义）
- 同一 Session 连多个 ccrpc
- 常驻进程 + unix socket 形态（一条连接管多个 Session）
- Workflow Run 内部结构事件
- 远程调用（经 ToH daemon）
- 多选题的 Other（自由文本）作答：未实测，目前返回 unsupported
- rewind / 编辑重发产生的 Branch：block 与 get_messages 目前按写入顺序，不区分 Main Line
- 切换 Session（/clear、/resume）时，切换被观察到之前已写入新 transcript 的内容块不发 block（只用于确认命令）
- 连续两次审批合并成同一段 waiting 的情形只做了逻辑处理，没有端到端用例
- Claude Viewer 的 Live 标记可以区分 waiting（进程文件 status=waiting + waitingFor），目前它把 busy 以外都当 idle
