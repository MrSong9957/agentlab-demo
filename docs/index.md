# 项目文档

规格按功能编号维护，写法见[规格文档实战法](规格文档实战法.md)。新证据放 `docs/logs/`；已有 `docs/records/` 保留历史引用。

当前顺序：先完成并验收 [024 原生模型接口](specs/024-native-model-apis.md)，再继续 [023 MCP 资源](specs/023-mcp-resources.md)和工具搜索等工具实现计划。各规格状态以正文为准。

| 规格 | 内容 |
|---|---|
| [001](specs/001-cli-repl-loop.md) | CLI 输入与轮循环 |
| [002](specs/002-llm-api-key.md) | 模型配置、凭证引用与目录 |
| [003](specs/003-exec-command.md) | 命令执行 |
| [004](specs/004-apply-patch.md) | 文件补丁 |
| [005](specs/005-minimal-prompt-assembly.md) | 提示词组装 |
| [006](specs/006-tool-call-loop.md) | 工具调用循环 |
| [007](specs/007-session-persistence.md) | 会话保存与恢复 |
| [008](specs/008-tui-rendering-port.md) | TUI 渲染接入 |
| [009](specs/009-project-layout.md) | 项目目录结构 |
| [010](specs/010-tui-selection-copy.md) | TUI 选择与复制 |
| [011](specs/011-tui-transcript-scrollbar.md) | 对话滚动条 |
| [012](specs/012-tui-tool-details.md) | 工具详情 |
| [013](specs/013-memory-system-v1.md) | 记忆系统 V1 |
| [014](specs/014-memory-minimal-revisions.md) | 记忆能力修订 |
| [015](specs/015-turn-context-injection.md) | 用户轮上下文注入 |
| [016](specs/016-process-sessions-plan.md) | 可续接进程会话 |
| [017](specs/017-update-plan.md) | 计划工具 |
| [018](specs/018-user-input.md) | 用户问询 |
| [019](specs/019-view-image.md) | 查看图片工具 |
| [020](specs/020-cli-image-input.md) | 用户图片输入 |
| [021](specs/021-send-message-to-user-async.md) | 执行中消息 |
| [022](specs/022-mcp-tools.md) | MCP 外部工具 |
| [023](specs/023-mcp-resources.md) | MCP 资源 |
| [024](specs/024-native-model-apis.md) | 原生多协议模型接入，当前优先 |
