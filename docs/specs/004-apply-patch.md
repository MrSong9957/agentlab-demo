# 规格：文本补丁修改（apply_patch）

> 状态：已实现（2026-10-08）　更新：2026-10-28（[029](029-file-patch.md) 交付 `Move to` 与第四级 Unicode 匹配容差，修订本页相应限制；其余契约不变）
> 定位：参考 Codex 编写的 MVP 规格；接口取舍见第 3 节。
> 来源：[openai/codex](https://github.com/openai/codex) · [初始工具定义](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/core/src/tools/handlers/apply_patch_spec.rs) · [补丁语法](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/core/assets/tools/apply_patch.lark) · [应用实现](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/apply-patch/src/lib.rs#L280-L590) · [行尾处理](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/apply-patch/src/text_file.rs)。源码固定在提交 `2fdf047c9631`。

## 1. 目标 & I/O

- 目标：根据补丁新增、局部修改或删除 UTF-8 文本文件，返回实际变更清单。
- 工具描述：接收 Codex 补丁格式，支持 `Add File / Update File / Delete File`；可涉及多个文件；`Update File` 块可带一行 `*** Move to:`（移动/重命名，029：结果记目标 `add`＋源 `delete` 两条）；仍不支持二进制文件。上下文匹配为四级容差（精确 → 忽略行尾空白 → 忽略两端空白 → 忽略两端空白＋Unicode 标点/空格映射，映射仅用于定位不改写文件；029 起）。
- 输入：补丁文本。Codex 原版直接接收自由文本；仅支持 JSON 函数工具的接口采用 `{"patch": "补丁文本"}`。路径基于会话当前目录解析，可使用相对或绝对路径；读写权限由执行环境控制。

示例：文件原来含有 `mode=dev`，调用后改成 `mode=test`。

```text
*** Begin Patch
*** Update File: config.txt
@@
-mode=dev
+mode=test
*** End Patch
```

- 输出：`{ok, changed_files, state_known, error}`，字段始终存在。`ok` 为是否全部完成的布尔值；`changed_files` 数组按执行顺序列出确认生效的 `{path, action}`，路径为实际目标的绝对路径，动作取 `add / update / delete`；`state_known` 为变更清单是否完整可信的布尔值，成功时为 `true`，失败目标可能被部分写入时为 `false`，须重读；`error` 成功时为 `null`，失败时为包含原因和可确定目标路径的字符串。

## 2. 边界 & 风险

- [ ] 格式错误、不支持的操作、空补丁。
- [ ] 新增目标已存在、修改/删除目标不存在、修改上下文匹配失败。
- [ ] 多文件补丁中途失败，前面的修改已经生效。
- [ ] 写入失败可能截断文件；行尾转换可能产生整文件差异。

## 3. 关键判断（为什么）

- 先完整解析，再按补丁顺序逐文件校验并执行；格式错误不写文件；更新先算出完整新内容再写，匹配失败不改当前文件。Agent 串行调用，修改前读取最新内容。
- 新增不得覆盖已有文件：这是 MVP 额外约束；Codex 的应用实现允许覆盖。修改文件必须使用 `Update File`，避免把新增当成覆盖。
- 失败停止，不自动回滚此前修改；返回确认生效的清单，写入结果不确定时标记 `state_known=false`。Codex 源码也记录失败前的已提交变更及其精确性；MVP 不承诺多文件事务。
- 行尾：参考 Codex，保留未改行的换行符；插入/替换行使用原文件首个换行风格，无原有换行时用 LF；新增文件用 LF；更新后补齐末尾换行。

## 4. 坑记录 → 实测后同步到个人知识库

以下是源码/设计提醒，尚未形成实测经验。

- 坑001：补丁应用失败，不等于所有文件都没变；重试前先核对清单和当前文件。
- 坑002：补丁格式正确，不等于上下文仍然匹配；重复内容应提供足够上下文定位。
- 坑003：原地写文件可能先截断再报错；不能把“写入返回失败”当作“原文件完整”。

## 5. 预测 → 实测

| 场景 | 预测 / 验收 | 实测 |
|---|---|---|
| 格式错误、空补丁、不支持的操作 | `ok=false`；清单为空；文件字节不变 | ✅ `invalid_patches_error_without_touching_files`：非法补丁组（空补丁、块头/块体错误、End Patch 后多余内容等）均 `ok=false`、清单空、`state_known=true`，探针文件字节不变、目录无新文件；JSON 参数非法 6 组另见 `invalid_json_args_error_without_side_effects`。029 起 `Move to` 为合法语法，该组改为覆盖其**非法变体**（重复/错位/空目标/位于 Add/Delete 块），含合法前序块整体不执行另见 `invalid_move_syntax_after_legal_block_executes_nothing` |
| 新增 → 修改 → 删除 | 内容和存在状态正确；清单动作准确 | ✅ `add_update_delete_full_cycle`：三文件字节与存在状态正确；清单按执行顺序给出绝对路径与 `update/add/delete` 动作 |
| 新增目标已存在 / 修改或删除目标不存在 / 更新上下文不存在 | 不改变当前目标；返回具体错误 | ✅ `conflicting_targets_rejected_without_changes`：五类冲突（含 `@@` 定位行不存在）均拒绝；错误含目标路径与不匹配内容；目标字节/存在状态不变；`state_known=true` |
| 两文件：第一项成功，第二项写入时故障 | `ok=false`；清单含第一项；`state_known=false`；重读第二项核对状态 | ✅ `second_file_write_failure_reports_partial_state` 两阶段确定性注入：0444 只读文件在 open 阶段被拒（目标字节不变）；RLIMIT_FSIZE=4096＋忽略 SIGXFSZ 使第二项**写入开始后** EFBIG 半写截断（实测目标恰为新内容前 4096 字节前缀，原内容已被 O_TRUNC 清除）。清单仅含已生效第一项；`state_known=false`；错误含路径与原因；重读两文件核对。定向变异 M-A（状态标记改坏）/ M-B（失败目标混入清单）均被该测试捕获 |
| LF、CRLF、混合行尾、无末尾换行 | 未改行保持原换行；新行沿用约定风格；更新补末尾换行 | ✅ `line_endings_follow_spec_rules`：六种行尾场景逐字节验证，含替换块中上下文行保留原行尾、无末尾换行补齐、新增文件 LF |

另覆盖：相对路径基于会话当前目录解析（`relative_paths_resolve_against_session_cwd`）；非法 UTF-8 修改目标拒绝且字节不变（`non_utf8_target_rejected_and_unicode_roundtrip`）；多 `@@` 块定位与纯插入块追加末尾（`multi_chunk_update_and_context_seek`）；只读目录/只读文件不可绕过（`readonly_permissions_are_enforced_not_bypassed`，权限门）。

验收：先写无效输入的失败测试，再实现；以上场景全部通过，同时核对文件字节、存在状态和错误后的实际变更。

## 固定边界（2026-10-10 安全修复 N5）

- 补丁文本：≤1 MiB（UTF-8 字节），在解析前检查，超限直接拒绝。
- 单文件读入与生成内容：均 ≤4 MiB（UTF-8 字节）且 ≤100000 逻辑行。逻辑行定义（2026-10-28 F-1 修订，与实现 `logical_lines`/`SourceFile::parse` 同源）：`\r`、`\r\n`、`\n` 均为行尾（CRLF 算一行；孤立 `\r` 也计一行——此前仅按 `\n` 切分致 lone-CR 文件漏计、绕过行数上限）；非空且无末尾换行的尾段算一行；空串 0 行。
- 读入按已打开 fd 的 fstat 预检＋有限读取（不得 metadata 检查后继续无上限 read_to_end）；行数在读入后、逐行展开前检查；生成内容在写入或截断目标前检查字节数与行数。超限操作不改变目标字节（校验型失败）；前序已成功操作仍遵守无回滚语义，changed_files/state_known 如实报告。
- O_NONBLOCK 打开、fd 判型、inode 绑定与写入复核（F2 收口）不变。
