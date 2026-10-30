# 035 多 Agent：规格依据

- 日期：2026-10-26；工作目录：`/workspace`。
- 授权范围：原表「多 Agent」一行，六工具统一形成[规格与实施计划](../../specs/035-multi-agent.md)，不拆六项、不进行功能实现。
- 本项为原表最后一行；029/030/031/032/034/035各项的文档规划不等于代码已实施或验收。本轮没有实际spawn子Agent、运行模型/MCP、构建或功能测试。
- 已先查看git status，保留既有AGENTS、文档迁移、029–034、033沙箱评估及配置相关改动，未暂存/提交/还原这些工作。

## 本地核对与设计约束

| 核对点 | 依据 | 对035的约束 |
|---|---|---|
| 现有工具循环 | [turn.rs](../../../src/app/src/turn.rs)固定8请求/32批次调用，模式/宿主控制与业务分派；[prompt.rs](../../../src/app/src/prompt.rs)的共同历史与原生包装 | 新增明确子轮模式，不调用会重置取消/读取真实用户输入的根宿主入口；同Agent顺序、跨Agent独立 |
| 搜索结果 | 在src/app/src、tests与Cargo文件查找六工具/Agent控制名称，只命中[memory/phase2.rs](../../../src/app/src/memory/phase2.rs)一条failed_spawn_agent母本注释 | 注释不是多Agent实现，当前没有六工具或调度器 |
| 模型发送 | [main.rs](../../../src/app/src/main.rs)持有连接/凭证/模型并构造sender；[tui_host.rs](../../../src/app/src/tui_host.rs)请求线程与等待侧取消；[api.rs](../../../src/app/src/api.rs)连接/总超时和10 MiB请求/响应边界 | 抽最小拥有型发送快照，不另写协议；取消等待不能声称HTTP线程已经退出，排空仍计槽 |
| MCP共享 | [manager.rs](../../../src/app/src/mcp/manager.rs)快照保留定义、原始身份、Arc连接，call/call_resource_tool逐次接收取消标志 | 可复用冻结快照和连接；不按名字拆路由，不因中断某Agent关闭共享服务器 |
| 进程 | [proc_session.rs](../../../src/app/src/proc_session.rs)当前一个sessions表、close终止全部、没有Agent owner；[016](../../specs/016-process-sessions-plan.md)有64总量/PTY/取消回收契约 | 新增可信owner及按owner回收，共享总容量，根原行为保留；子任务不得用全局close清理自己 |
| 会话与记忆 | [session.rs](../../../src/app/src/session.rs)只有V1/V2；[rollout_input.rs](../../../src/app/src/memory/rollout_input.rs)有独立版本/预算；[runner.rs](../../../src/app/src/memory/runner.rs)按根宿主启动管线/登记durable来源 | 内层记录不伪造根tool_calls；基于032 V3/保存回执单写者提交，记忆只登记根，不把fork复制当新经历 |
| 其他规划 | [030](../../specs/030-tool-search.md)、[031](../../specs/031-request-permissions.md)、[032](../../specs/032-context-management.md)、[034](../../specs/034-javascript-tool-orchestration.md)均待实现；[033](../../specs/033-tool-sandbox.md)为独立评估 | 032基建为实施前置；034不依赖/不承载六工具；目录和临时权限不经委派扩大，本轮不改其他规格/环境 |

## 实际读取的固定上游

GitHub连接器按 `openai/codex@d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4` 读取：

- [multi_agents_spec.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/multi_agents_spec.rs)同时定义旧V1与V2。V2使用task_name/message、all/none、消息/续派、中断和路径列表；本项沿原表六工具，不同时搬旧send_input/resume/close。输出结构、等待上下限、层级/模型覆盖和跨轮生命周期均按本地规格，不能直接假定相同。
- [multi_agents_v2.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/multi_agents_v2.rs)注册六个handler；[message_tool.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/multi_agents_v2/message_tool.rs)中send_message/followup共用发送路径，以delivery mode区分触发轮。本项目据此把“入队”与“唤醒”分开，不把受理回执当任务完成。
- [wait.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/multi_agents_v2/wait.rs)观察邮箱、steer和超时，其参数受配置限制；本项选择0–60000ms、默认30000ms，并在合法批末交正文，是明确的本地适配。
- [registry.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/agent/registry.rs)有路径、并发槽的保留/提交/释放；[status.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/agent/status.rs)从真实事件推导运行/完成/中断状态。本项参考原子预留和事实驱动状态，不搬整个Codex Session框架或凭名称实现假调度。

已实际打开[OpenAI Docs的Responses多Agent说明](https://developers.openai.com/api/docs/guides/responses-multi-agent)，确认其六个协作动作由服务端执行、multi_agent_call/output和agent_message是托管协议路径。本项目选择五协议普通函数＋本地调度，不开启托管模式、不自己执行托管动作，也不沿用其默认并发/深度/模型可用性承诺。

## 方案与未测事项

一层/3子执行槽、同根轮生存期、根8＋子累计24次请求、消息不自动唤醒、owner进程回收、V3来源事件及预算预留均为035首版方案。接口、状态/额度、性能测法和验收以规格为唯一清单，此处不复制。

没有并发原型、真实子轮、消息/终文竞态、传输排空、跨Agent资源隔离、V3落盘/恢复或费用实测。M0须先核对032基建交付、冻结性能/资源要求；不能因为文档已有状态机和工具名就标已实现。

## 文档验证

内容、命名、五节、六工具、计划、待测用例、JSON、本地链接、原表项目顺序与本轮修改范围的检查见终端日志。日志记录实际命令、工作目录、退出码和原始输出；文档检查不代表模型/功能/性能/安全验收通过。
