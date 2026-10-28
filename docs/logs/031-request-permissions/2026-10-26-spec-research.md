# 031 权限申请：规格依据

- 日期：2026-10-26；工作目录：`/workspace`。
- 本轮授权：原表下一项「权限申请」的[规格与实施计划](../../specs/031-request-permissions.md)。只做文档，不实现、不安装、不改管理员配置或真实凭证，不推进「上下文管理」。
- 状态：待实现。当前结论来自源码/既有规格静态核对，未运行本项沙箱、权限或性能探针。

## 当前实现核对

| 核对点 | 源码/文档证据 | 规划含义 |
|---|---|---|
| 原表 | [权限申请一行](../../records/2026-10-21-codex-tools-comparison.md)明确缺 request_permissions 且需配套执行环境 | 权限接口、确认、后端和收尾不能分成多个后续任务或只交空工具 |
| 027现状 | [027规格](../../specs/027-credential-access-boundary.md)标明产品降权链路存在、全链路管理员部署仍有前提；[catalog.rs](../../../src/app/src/catalog.rs)有隔离目标与TRUST_CONTROL_VARS | 不能把已有sudo路径当作按路径/网络沙箱；031启用前须有真实身份和部署证据 |
| shell与会话 | [proc_session.rs](../../../src/app/src/proc_session.rs)的build_tool_command/spawn_piped/spawn_pty及[016](../../specs/016-process-sessions-plan.md)保留跨轮活进程 | 临时授权必须标记启动快照、到期杀完整树；基线进程继续既有语义，不能无说明改变全部进程 |
| 补丁 | [turn.rs](../../../src/app/src/turn.rs)隔离分派＋[apply_patch.rs](../../../src/app/src/apply_patch.rs)的internal-apply已把读判型写移入子进程 | 可以继续在子进程外套固定OS后端，不能主进程先读模型源路径 |
| 看图 | [view_image.rs](../../../src/app/src/view_image.rs)的prepare_from_path当前在宿主打开、读取并解码 | 只约束shell会漏掉文件读取面；031需要把模型看图的读取移入受限内部入口 |
| MCP | [transport.rs](../../../src/app/src/mcp/transport.rs)stdio已有降权及env_clear；[022](../../specs/022-mcp-tools.md)连接跨轮复用、HTTP身份由配置确定 | stdio只用基线，不动态重启或继承临时许可；主进程模型/MCP HTTP控制面与本地shell网络分开 |
| 确认通道 | [user_input.rs](../../../src/app/src/user_input.rs)有单次专用通道/取消；[main.rs](../../../src/app/src/main.rs)及[tui_host.rs](../../../src/app/src/tui_host.rs)接入宿主 | 复用输入者和界面构件，新增专用权限回执；普通问询回答和聊天不能直接变成授权 |
| 展示占位 | [codex-protocol shim](../../../src/shims/codex-protocol/src/lib.rs)的ActivePermissionProfile空枚举、FileSystemSandboxEntry空struct以及PermissionProfile展示类型 | 不计为实际权限或沙箱实现，不能据此把工具表标已实现 |
| 容器与平台 | [compose](../../../.devcontainer/compose.yaml)、[Dockerfile](../../../.devcontainer/Dockerfile)未配置本项权限助手；本会话早先普通命令由工具层报bwrap命名空间错误 | 不把工具层错误外推成产品探测；M0需实际目标环境验证，不将未配置后端说成已经可用 |

## 实际读取的固定参考

使用GitHub连接器按 `openai/codex@d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4` 读取，没有以HEAD替代：

- [handler](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/request_permissions.rs)、[协议](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/protocol/src/request_permissions.rs)、[shell_spec](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/shell_spec.rs)：确认额外filesystem/network、reason、等待宿主、turn/session scope及后续执行生效。031明确收窄为单环境、turn、全允许/全拒绝，保留同名工具并记录参数差异。
- [models.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/protocol/src/models.rs)：核对file_system.entries和network.enabled结构，没有照搬旧read/write数组。
- [Linux helper README](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/linux-sandbox/README.md)、[Cargo.toml](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/linux-sandbox/Cargo.toml)：确认独立可执行文件、bubblewrap/seccomp与源码依赖边界，选择外部固定助手，避免引入整个上游workspace。
- [真实CLI](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/linux-sandbox/src/linux_run_main.rs)和[bwrap构造](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/linux-sandbox/src/bwrap.rs)：确认permission-profile/cwd参数、旧Landlock及no-proc/inherit-pid开关、new-session/die-with-parent。上游README写有旧/proc回退，本规格明确拒绝该回退并要求逐启动校验；信号和PTY不能只看源码就算实测通过。
- 已实际打开的 [OpenAI Docs：批准与安全](https://learn.chatgpt.com/docs/agent-approvals-security)、[权限后端](https://learn.chatgpt.com/docs/permissions)及 [bubblewrap官方说明](https://github.com/containers/bubblewrap)用于核对批准/强制执行的区别和调用方策略责任；没有把通用工具或成熟库当成已经完成本项目隔离的证明。

## 取舍与仍未验证的条件

具体接口、边界和验收统一留在规格中。本轮选择Linux固定后端、可信启动策略、027身份前提和本轮授权；不触发root操作或撤销现有用户授权。策略/助手安装及027管理员部署需在实施时提供可审阅材料；当前没有这些条件已满足的证据。

性能和IPC/PTY/信号尚未运行。M0必须固定助手及运行环境，用假数据证明拒绝/允许/到期后拒绝，再冻结资源阈值；仅验证“不支持时拒绝”不能代替支持环境中的完整正例验收。

## 文档验证

本轮只校验项目映射、编号、五节结构、实施计划、用例待测状态、JSON示例、链接与空白。真实命令、工作目录、退出码及原始输出见 [2026-10-26-doc-check.log](2026-10-26-doc-check.log)。未运行Rust构建、代码测试、权限攻防或真实沙箱探针。
