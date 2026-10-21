# 2026-10-05 安全验证证据

保留原始结构化结果和修复前后捕获，文件字节不变。完整原目录、历史攻防脚本`attack.py`及空捕获占位由Git快照`9e32a63`保存；见[原件索引](../../../records/evidence-index.md#security-originals)和[逐文件清单](../../../testing/evidence/curation-manifest.json)。历史脚本不是当前测试入口。

- [results.json](results.json)：最终修复后38条，权威结果；`leak_check`为空。
- [results-prefix-fix.json](results-prefix-fix.json)：按原样保存的定点结果，不单独代表整轮覆盖。
- [v1-prefix-evidence/results-final-check.json](v1-prefix-evidence/results-final-check.json)：历史捕获中的结构化检查。
- [security-test-log.md](security-test-log.md)：原始设计依据、失败与修订记录，保持原文。
- 其余`.txt`：请求、拒绝／放行及修复前后实际输出，保留各自来源上下文，不能仅因内容相同就视为同一次执行。

本目录由不同轮次的捕获组成；文件名中的“v1”不代替具体报告对轮次和修订的界定。历史`logs/`及旧`docs/records/`路径需通过当前清单或快照解析，不把当时命令当作当前仓库的可执行示例。

结论见[原安全报告](../../reports/2026-10-05-terminal-chat-api-security.md)及[修复报告](../../reports/2026-10-05-terminal-chat-api-security-fix.md)。本次没有重新运行攻防脚本或修改安全验证结论。

原件结构核对：`results-prefix-fix.json`实际含1条，`v1-prefix-evidence/results-final-check.json`实际含38条；此处只说明文件内容，不改写历史报告的覆盖口径。
