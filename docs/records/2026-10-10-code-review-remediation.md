# Code-review 整改（2026-10-10）

> 对象：同日 code-review（Standards/Spec 双轴）四组发现。原则：只做必要修改，先核对实际
> 代码再动手，误报记录不硬改。基线：枚举 215，`cargo check` 0 警告（退出码 0），
> `cargo test --workspace --no-fail-fast` 215 通过 / 0 失败 / 0 跳过（退出码 0）。
> 未提交未 push。

## 组 1（Standards-1）workdir 测试隔离【已整改】

- **核实**：`workdir_resolves_relative_paths` 确在测试进程 cwd（仓库根）创建
  `tchat-relwd-<pid>` 目录与文件——§3 违规属实，非误报。
- **整改**（仅测试夹具，exec 生产代码零改动）：复用 EFBIG 子进程重入模式——父测试持有
  外部临时会话目录，以 `--exact` 重入测试二进制并把子进程 `current_dir` 设为该目录
  （`workdir_child_mode`：`TCHAT_WORKDIR_REL`/`TCHAT_WORKDIR_OUTCOME` env 触发）；子进程
  在会话目录内以相对 workdir 调用真实 `exec_command` 并执行无 workdir 的 `pwd > 文件`。
  断言保留并加强：相对场景读到的内容/退出码/无错误＋子进程创建的输入文件；缺省场景以
  “pwd 落盘于会话目录且内容即该目录（canonicalize 对照）”证明缺省＝会话当前目录
  （原为只读 `test -f Cargo.toml`，不依赖仓库布局）。未改测试进程全局 cwd；取证在
  `remove_dir_all` 之前完成，断言失败也无残留。无 RED（夹具整改）；夹具首轮自纠一处
  取证顺序错误（pwd canonicalize 误置于清理后），修正后通过。

## 组 2（Spec-c2）tool_calls id/name 原样保留【属实，RED→GREEN】

- **核实**：`parse_tool_calls` 确对 id/name 做 `str::trim`（存储/唯一性/分派均用 trim 后
  值）——006 §1“原样保留”冲突属实，非误报。
- **RED（业务断言失败，均以全名 `--exact` 实跑确认执行数＝1）**：
  `tool_call_id_and_name_preserved_verbatim`——left `"id-a"` ≠ right `" id-a "`（assistant
  记录被 trim）；`tool_call_id_uniqueness_uses_original_values`——原始 id 不同、trim 后相同
  的两项被判“id 重复”整批拒绝。
- **最小修复**：提取时去掉 `.map(str::trim)`；`id.trim().is_empty()`/`name.trim().is_empty()`
  仅作全空白判断；存储、`seen` 唯一性、`execute_tool`/`skipped_by_cancel` 分派均用原值。
  不新增名字字符限制，不改写 arguments。
- **GREEN＋行为锁定**：id 含空白→assistant 记录、tool_call_id、续请求投影逐字一致且该
  调用正常执行；name `" exec_command "`→未知工具单 error 结构（错误含原始名字）、探针不
  创建；原始 id 重复、id/name 全空白仍整批拒绝（既有 parse 测试＋新断言）。e2e
  n2b/n2b2 回归绿。

## 组 3（Standards-3＋Spec-c3）结果结构同源＋取消未知工具通用结构【RED→GREEN＋重构验证】

- **核实**：`turn.rs` 三处手写 ExecResult/ApplyPatchResult JSON 字面量属实；取消时未知
  工具落 exec 形态（`_` 分支）属实。两结构体均“字段始终存在、无 skip 属性”，结构体序列化
  与原字面量**字节等价**（以既有 `result_serializes_spec_shape` 锚定为据）。
- **RED**：`cancelled_batch_skips_per_tool_shapes_and_unknown_generic_error`——未知工具
  未执行结果拿到五字段 exec 形态，期望单 error 字段（确定性触发：第一条 apply_patch 的
  notify 回调置取消标志，无计时依赖）。
- **修复**：`skipped_by_cancel` 与 `execute_tool` 参数错误分支改为构造 `ExecResult`/
  `ApplyPatchResult` 结构体序列化；未知工具取消分支回传 `{"error":"用户取消，本调用未执行"}`。
  已知工具字段/取值/错误语义不变（新测试对 exec/apply_patch 取消形态做全字段精确对照；
  n2b2 真实 Ctrl+C 混合批回归绿）。apply_patch 无回滚与 state_known 语义未触碰。
- **重构验证**：结构体化改动无行为差异，由全字段精确断言＋既有 t5/n2b/n2b2 证明。

## 组 4（契约文档同步）

- **核实（部分误报）**：审查聚合注称“AGENTS.md §7 尚未补 F1 与 N1 CR 剥离”——实际 §7
  已载 CR 剥离（固定边界句）与凭证脱敏（`redact_credential`，2026-10-10 高风险清单轮
  注记已在）；**仅 F1 信任根围栏与 006 新契约确实缺失**。Spec 轴 (b)1/(b)3 的“规格外”
  定性同样源于阅读清单未含安全报告——F1/N1/redaction 均为已批准修复轮（报告在
  docs/security/reports/），非擅自扩权。
- **同步**：AGENTS.md §7 补 F1 信任根变量围栏（变量名单＋大小写不敏感＋报告引用）与
  006 id/name 原样保留、取消未知工具通用结构；006 规格三处——发送层入口改为“返回完整
  响应 JSON（`chat_with_body`），`choices[0].message` 由 turn 提取”、协议校验句补原样
  保留语义、固定边界补未知工具取消结构。

## 审查发现处置对照（范围保护）

| 发现 | 处置 |
|---|---|
| Standards-1 workdir 隔离 | 已整改（组 1） |
| Standards-3 手写结果 JSON | 已整改（组 3，字节等价重构） |
| Spec-c2 id/name trim | 已整改（组 2） |
| Spec-c3 取消未知工具形态 | 已整改（组 3） |
| Spec-c1 发送层入口描述 | 文档同步（组 4） |
| Spec-b1/b2/b3、AGENTS 滞后 | F1 与 006 补齐；CR/redaction 系审查部分误报（§7 已载） |
| Standards-2/5 发送错误映射、chat 委托重复 | 本轮明确不处理（保留 `api::chat` 及其兼容行为） |
| Standards-4 测试夹具共享下沉 | 不处理（避免无关重构） |
| Standards-6 `api::chat` 仅测试可达 | 保留（用户要求） |
| Standards-7 exec 测试固定 sleep | 不处理（既有约定，未扩大） |
| Standards-8 `expected_at_at` 命名 | 不处理（变量改名属无关清理） |

## 终态验证（2026-10-10 实测）

- 枚举差集（程序比对）：新增恰 3 项（`turn::tests::tool_call_id_and_name_preserved_verbatim`、
  `tool_call_id_uniqueness_uses_original_values`、`cancelled_batch_skips_per_tool_shapes_and_unknown_generic_error`），
  删除 0——无意外删合；215→218。
- 定向验证均以全名 `--exact` 实跑、执行数＝1：三个新测试 RED（预期业务断言）→修复→GREEN；
  `workdir_resolves_relative_paths` 夹具整改后直接 GREEN；`n2b`/`n2b2`（真实 Ctrl+C）回归绿。
- `cargo check`：0 警告，退出码 0（既有 4 个测试目标警告系此前轮次遗留，本轮未触碰未新增）。
- `cargo test --workspace --no-fail-fast`：**218 通过 / 0 失败 / 0 跳过**，退出码 0
  （lib 87＋apply_patch 35＋e2e_acceptance 11＋exec_command 16＋llm_flow 49＋
  prompt_assembly 6＋tool_loop 14）。
- `api::chat` 保留未动；未提交、未 push。
