# apply_patch 与 Codex 的差异复核

- 日期：2026-10-22。
- 范围：[工具对比记录](2026-10-21-codex-tools-comparison.md)第二项「文件补丁」。
- 上游基线：沿用原记录的 `openai/codex@d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4`，通过本地缓存的 `git show <提交>:<路径>` 读取，未使用缓存当前 HEAD 代替。
- 状态：源码、规格和已有测试复核；本次仅调整文档。

## 结论

当前无需紧急修改代码，继续复用现有实现。原表中的「参数与补丁语法仍有差异」需要细化：JSON 参数属于主动适配；移动文件和 Unicode 标点匹配容差属于 MVP 未移植能力；新增文件禁止覆盖属于应保留的约束。它们不等同于现有工具契约失效。

本项目的实际契约是 [004 文本补丁规格](../specs/004-apply-patch.md)，并未承诺完整兼容 Codex。仅凭同名工具存在差异，不足以要求整体替换实现。

## 差异、影响与处理建议

| 项目 | Codex 固定版本 | 本项目现状与影响 | 建议 |
|---|---|---|---|
| 调用参数 | 自由文本工具，直接提交补丁，不包 JSON | Chat Completions 函数工具，只接受 `{"patch":"补丁文本"}`；工具定义与发送版提示词示例均已适配 | 保留 `patch` 参数；无需为名称一致改成自由文本或改动 API 协议 |
| 基础语法 | `*** Begin Patch` / `*** End Patch`、Add/Update/Delete、`@@` 上下文、`*** End of File` | 已实现这些基础格式；并非另用一套 unified diff 语法 | 继续复用；不能据此宣称支持所有 Codex 补丁 |
| 移动与重命名 | Update 块支持 `*** Move to: ...`，可同时修改内容 | 明确拒绝 `Move to`；模型若照搬此类上游补丁，本次调用会失败，完整解析阶段尚未执行文件操作 | 有直接接收上游移动补丁的需求时，优先补此项；先修订 004，明确目标冲突、失败后清单、展示和会话兼容语义 |
| 上下文匹配 | 精确、忽略行尾空白、忽略两端空白后，增加 Unicode 标点与空格归一化 | 只保留前三层；弯引号与直引号、长横线与短横线等不一致时，可能匹配失败；UTF-8 文本本身受支持 | 暂缓；出现相关失败时再评估复用上游归一化逻辑，当前可重读原文后按实际字符生成补丁 |
| 新增已有目标 | Add 写入实现允许覆盖已有文件 | `create_new` 拒绝已有目标，包括符号链接；已有文件应使用 Update | 保留保护，不随语法兼容改造放宽 |
| 结果契约 | 上游补丁执行结果与宿主工具输出配套 | 返回固定四字段 `ok / changed_files / state_known / error`，并向展示层提供真实前后差异 | 保留本地契约；后续移植须继续如实报告部分成功和不确定写入 |

OpenAI 官方文档推荐复用其补丁格式及参考实现，以适配模型训练中的格式；这支持按需补齐语法，不能据此推导出本项目必须更换现有 JSON 封装。[Codex Prompting Guide](https://developers.openai.com/cookbook/examples/gpt-5/codex_prompting_guide)

若后续明确要求「直接消费 Codex 生成的完整补丁」，当前实现不满足该目标，应另立行为变更任务；本次检查不将这一潜在需求视为已批准规格。

## 核对依据与验证范围

- 上游固定提交：`codex-rs/core/src/tools/handlers/apply_patch_spec.rs`、`codex-rs/core/assets/tools/apply_patch.lark`、`codex-rs/apply-patch/src/seek_sequence.rs`、`codex-rs/apply-patch/src/lib.rs`。分别核对自由文本定义、Move to 语法、第四级匹配容差与覆盖写入实现。
- [工具定义和提示词适配](../../src/app/src/prompt.rs)：`apply_patch_tool` 只声明必填字符串 `patch` 与 Add/Update/Delete；`rewrite_apply_patch_example` 将母本旧调用示例改成相同的 JSON 参数。
- [补丁实现](../../src/app/src/apply_patch.rs)：`ApplyPatchArgs::from_value` 拒绝未知字段；`update_lines` 明确拒绝 Move to；`seek_sequence` 仅三级容差；Add 使用 `create_new`。
- [请求组装测试](../../src/app/tests/prompt_assembly.rs)：已存在 `s2_sent_prompt_only_listed_adaptations` 和 `s6_tool_whitelist_matches_local_contracts`，检查发送文本、参数 Schema 与本地解析器契约。
- [文件系统测试](../../src/app/tests/apply_patch.rs)：已存在 `invalid_patches_error_without_touching_files`（含 Move to）、`invalid_json_args_error_without_side_effects`、`conflicting_targets_rejected_without_changes` 与 `add_update_delete_full_cycle`。这是已有测试覆盖的源码证据，不是本次运行结果。
- 文档验证通过：本次涉及的 3 份文档共 16 条本地链接均可解析，新记录命名、UTF-8、末尾换行和空白检查通过，`git diff --check` 通过。本次未运行代码检查或全量测试，未测量真实模型调用成功率。
