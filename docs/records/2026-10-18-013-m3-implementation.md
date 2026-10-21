# 013-M3 第一阶段提炼实施记录（2026-10-18）

任务分支 `feat/013-memory-m3`（基线 main @ `651bbdf`）。上游锚：openai/codex @
`ce0323f2f2d3b25e82806cd555237da3db4e1897`——`memories/write/src/{rollout_input.rs,
phase1.rs,phase1_output.rs,prompts.rs}`、`memories/write/templates/memories/
{stage_one_system,stage_one_input}.md`（逐字节拷贝，sha256 一致）、
`secrets/src/sanitizer.rs`、`utils/string/src/truncate.rs`（经 008 shim 复用）、
`utils/output-truncation`/`protocol` 的 `TruncationPolicy`、`write/src/lib.rs mod
stage_one` 常量。实施前逐项核实（/tmp/codex-m3/ 本地参考）：三态判定式
（`raw_memory some_and(is_empty) || rollout_summary.is_empty`，空字符串判定非空白）、
预算公式（有效窗口 70% `.max(1)`/回退 150k/`APPROX_BYTES_PER_TOKEN=4`）、
retry 3,600s、slug 可 null 可缺省（serde default）。

## 1. 变更清单

- `src/app/src/memory/secrets.rs`（新）：`redact_secrets` 四正则逐字移植
  （regex→regex-lite——工作区已锁、tui 同款；兼容性以内联移植测试证明：上游
  两测试＋三正则行为锚，D7 条件满足，未引入 `regex` crate）。
- `src/app/src/memory/truncate.rs`（新，收窄）：`TruncationPolicy`
  （Bytes/Tokens＋预算换算）＋`truncate_text`；底层中间截断/近似换算**复用
  008 shim `codex-utils-string`**（上游同文件逐字节，code-review 硬违规整改：
  初版重复移植约 120 行已删）。
- `src/app/src/memory/rollout_input.rs`（新）：`read_recent_rollout_items`
  （头部 meta 校验→分块反向扫描→超限/坏行跳过→8 MiB 预算新→旧→恢复来源
  顺序）＋`serialize_filtered_history_items`（过滤注入块→`project_history`
  投影→JSON 数组→整体脱敏；`display` 展示数据结构性排除）。
- `src/app/src/memory/phase1_output.rs`（新）：V1 输出合同解析
  （deny_unknown_fields＋slug 可缺省；三字段逐一脱敏）。
- `src/app/src/memory/phase1.rs`（新）：`run_stage1_job` 三态编排（读取→
  序列化→组装→模型闭包→`api::reply_text` 提取→解析→三态提交，失权拒提交＝
  Failed）；`rollout_token_limit`/`build_stage_one_input_message`（Tokens 中间
  截断保首尾）／`build_stage_one_request`（system＝模板原文＋user＝渲染输入，
  无 tools）；常量 `JOB_RETRY_DELAY_SECONDS=3_600`/`DEFAULT_ROLLOUT_TOKEN_LIMIT
  =150_000`/`STAGE_ONE_SYSTEM`。
- `src/app/src/memory/templates/`（新）：上游两模板逐字节拷贝（Apache-2.0，
  来源登记于 phase1.rs/规格；渲染＝单遍占位符替换，元数据先行内容最后，
  内容中模板样式文本不二次替换——有测试钉住）。
- `src/app/src/memory/mod.rs`：模块声明与再导出。
- `src/app/src/prompt.rs`：`project_history` 改 `pub(crate)`（提炼输入复用
  005 投影，单一来源；`display` 排除契约同一处生效）。
- `src/app/src/api.rs`：`chat` 的回复提取抽为 `pub(crate) fn reply_text`
  （与 M3 采样共用，错误文案单一来源；行为零变化，llm_flow 51/51 复绿）。
- `src/app/Cargo.toml`：＋`regex-lite`（无新锁条目）＋`codex-utils-string`
  path 依赖（复用 008 shim）。
- `src/app/tests/memory_phase1.rs`（新，17 用例）。

D6（用户已确认 2026-10-18）：默认当前 `selected_model`、无 `extract_model`
配置——请求组装经注入闭包携带模型（生产接线 M7 经 `chat_with_body`）；
本轮全部 mock（闭包＋本地 HTTP mock 双路径），未调用真实模型。

## 2. TDD 证据

- 基线：`cargo check` 通过；memory 集成 18+21、lib 102 全绿（全量 2817/0 有案）。
- RED：先写 `memory_phase1.rs` 全部用例（引用未实现符号）——E0432/E0433/E0425
  编译失败；其间修一处测试辅助笔误（badmeta 用例应写序列化行）。
- GREEN 过程中两个测试侧 bug（非实现 bug）修正：①`claim_direct` 辅助把
  session_path 写成假路径（读取必失败——`read_and_header_failures` 恰因期望
  失败而误过，其余全挂）→改为传真实路径；②单测 UTF-8 断言含标记字符、
  预算算术不足 8 MiB 两处测试笔误。修正后集成 17/17（补强后）＋模块单测全绿。

## 3. comprehensive-testing（Skill 已于本会话装载，按其工作流）

审计缺口→补 4 项：失败原因入库前脱敏（闭包回显 `sk-` 密钥→`last_error`
含 `[REDACTED_SECRET]`）；超限记录居中时跳过后继续向前扫描（旧记录仍可入集）；
超限 meta 头整拒；失权所有者**失败记账**同样被拒（任务行保持新所有者 done，
Failed 结果≠污染新行——区分「结果 Failed」与「行被改 error」）。另有目录项
`dir.jsonl` 存在性≠可装载用例（Security Gate 输入）。拒绝项：时间资格 vs
内容价值＝结构性分离（M2 窗口用例 vs M3 空结果判定，各自独立已覆盖）；
`AssistantToolCalls` 投影形状（005 既有测试覆盖）。

**变异证明（断言不变，临时破坏）**：①序列化不脱敏→A13 输入面挂；②
`deny_unknown_fields` 移除→合同拒绝用例挂；③空结果按有效结果提交空串→
A12 两用例挂；④预算不做 70%→token_limit 两用例挂；⑤读取预算检查移除→
8 MiB 总量用例挂；⑥来源顺序不恢复→顺序用例挂。全部恢复后复绿。

## 4. Security Testing Gate（本轮进入——新增「会话内容→模型请求」与
「提炼结果→存储」两条数据流；无已装 Skill，按语义执行）

| 面 | 证据 |
|---|---|
| 脱敏（A13 三位置＋失败信息） | 输入面：请求体不含原始密钥、含 `[REDACTED_SECRET]`；存储面：`stage1_outputs` 三字段脱敏；解析面：`StageOneOutput::parse` 直测；失败面：`last_error` 脱敏（m3_failure_reason_redacted_before_storing） |
| 输入限制 | 8 MiB 总量（多块用例＋总量断言）；1 MiB/条（居中超限续扫用例）；头部限内（超限 meta 整拒）；模型预算中间截断保首尾 |
| 失败信息 | 来源均为有界错误（api 层 300 字符惯例＋serde 错误不含输入回显）＋入库前 `redact_secrets` |
| 参数绑定 | M3 零新增 SQL（store 层未动，全部 M1 已门禁原语）；读取为有界 I/O（take(MAX+1)/分块） |
| 旧所有者写入保护 | A09/A10：租期接管后旧令牌成功提交拒、迟到提交拒、**失败记账拒**（行保持新所有者状态） |
| 装载边界 | 路径＝M2 readdir∩stem 构造（stem 不含分隔符，结构性无穿越）；实际打开＋meta 校验替代存在性（目录项 `dir.jsonl` 用例→Failed）；符号链接跟随＝本地用户信任域（上游同位，M5 `mem_*` 围栏属合并轮） |

## 5. code-review（Skill 双轴，并行子代理）发现与处置

- **Standards 轴硬违规（已修）**：`truncate.rs` 与 008 shim
  `codex-utils-string`（上游同文件逐字节）重复移植——改为 app path 依赖＋
  本模块收窄为 `TruncationPolicy`/`truncate_text`（重复 ~120 行删除；shim
  自带上游测试锚，工作区门禁覆盖）。
- 判断题处置：`extract_reply_content` 与 `api::chat` 重复→抽 `api::reply_text`
  共享（llm_flow 51/51 复绿）；测试设施重复→`memory_phase1` 改用
  `common::TempDir`、`rollout_input::Tmp` 加纳秒防残留踩旧；`SteppedClock`
  两份保留（各集成目标自含辅助＝仓库现状惯例，登记）；`mod.rs` 再导出面
  宽（M7 消费＋Rust 惯例）保留。
- **Spec 轴**：无实质缺失/无蔓延；两处已登记偏离补录——⑥失败原因脱敏
  （上游不脱敏，本地加严）、过滤时机（上游读取侧剔除不占预算，本地序列化
  侧过滤已计入——本地会话结构性无注入块，影响为零）；`rollout_token_limit`
  少乘 `effective_context_window_percent`（上游默认 100%，等价，已登记）；
  A13「导出」面本地以解析面替代（M3 无导出载体），**M4 Markdown 同步须补
  脱敏断言闭环**（规格 M4 条目待注）。

## 6. verify:full 与最终验证

- `cargo check`：通过（exit 0；仅 tui 层既有警告）。
- `cargo test --workspace --no-fail-fast`：**2854 通过 / 0 失败 / 0 忽略**（exit 0；
  M2 基线 2817＋M3 新增 37＝集成 17（memory_phase1）＋模块单测 20
  （secrets 5/truncate 3/rollout_input 6/phase1_output 3/phase1 3），逐条对账无缺口）。
- 审查整改后受影响检查复跑：check 0 错、lib 122/122、api 3/3、llm_flow 51/51、
  memory 集成 17＋18＋21 全绿；验证后仅文档回填，无影响结果的代码改动。

## 6a. 定点补验轮（2026-10-18，交付前）

**背景**：①「磁盘级 I/O 失败未模拟」风险以失权拒提交替代——两种路径不能
等同；②「openai-chat 无 output_schema 端点」的协议表述不准确。

**存储 I/O 故障补验（新增 2 用例，直接通过＝补验，不主张 RED）**：
注入方式＝目录改名（SQLite journal 由主库路径派生，父目录失联后任何写事务
journal 创建即 ENOENT——对 root 与普通用户同样确定性生效；读取经既有 fd
不受影响；恢复守卫保证 TempDir 清理）。与失权的区别：失权＝SQL 条件不匹配
（`rows_affected=0`，语义拒）；I/O＝SQL 执行错误（`Err`，事务回滚）。
- `m3_storage_io_error_on_success_commit_preserves_transaction_contract`：
  采样成功＋提交写入故障 → 传播为 Failed（不报成功）；无部分提交——任务行
  保持 running、水位未推进、重试计数不消耗（存储错误≠业务失败）、租期保持、
  结果行未写、全局变化信号未发；且请求确已发出（失败定位在存储层，注入
  有效性由构造自证：若注入不生效结果会是 SucceededWithOutput）。
- `m3_storage_io_error_on_failure_accounting_swallowed_gracefully`：采样失败
  ＋记账写入同时故障 → 记账错误按上游 `let _ = mark_stage1_job_failed` 语义
  吞没，结果仍如实 Failed、任务行原状（running/无 retry_at/计数 3/无
  last_error）、不 panic。
- 既有 M1 `transaction_failure_rolls_back_all_writes`＝store 内部 SQL 级冲突
  回滚（非编排面、非 I/O 级），与本轮用例互补；实现无缺陷发现，无需修复。

**结构化输出适配核准（零契约变更，仅修正说明与证据）**：
- 实际请求体（mock 捕获断言，`m3_selected_model…` 扩展）：无
  `response_format`/`output_schema`/`tools`；system＝上游模板原文（含
  "Return exactly one JSON object with required keys"/"No additional keys"）；
  user＝渲染输入（点名三契约键）。
- 协议事实（OpenAI 官方文档 developers.openai.com/api/docs/guides/
  structured-outputs）：Chat Completions 对应机制＝`response_format:
  {type:"json_schema", json_schema:{name, schema, strict}}`（`name/schema/
  strict` 位于 `response_format.json_schema` 内），strict 保证 schema 遵循；
  **仅较新模型支持**（gpt-4o-2024-08-06 起）。Responses API 的 HTTP 请求
  字段是 `text.format`；Codex 源码中的 `output_schema`（`Prompt.output_schema`
  ＋`output_schema_strict`）是其内部参数层级，不是 HTTP 字段（勘误：前文
  「上游 Responses API 用 output_schema」表述不准，本段为准）。
- 分层保证：请求侧＝模板原文引导（尽力而为）；响应侧＝`phase1_output`
  严格解析（deny_unknown_fields＋类型必检）拒绝一切违规→失败＋重试（A11），
  违规输出永远进不了存储。与上游差别＝违规输出成本一次模型调用而非被端点
  阻止（登记差异）。用户可经既有 `request_params`（`supported_params` 门，
  `checked_params` 逐键校验后并入请求体）自行透传 `response_format`——M3
  不添加也不禁止。供应商实测待 M7。
- phase1.rs 头注释②按上述重写（原「无本地协议面」表述不准确）。

**本轮范围复核**：变化＝2 新用例＋请求体断言扩展＋phase1.rs 注释（零生产
逻辑改动）——Security Gate 面无新增（测试内注入、临时目录内）；审查复核＝
注释与实测行为一致性（已核对）。L1/L2：memory_phase1 19/19、memory 单测
22/22、api 3/3、llm_flow 51/51、candidates 18＋store 21 全绿；L3 见下。
- L3 补验轮全量门禁：`cargo check` exit 0；`cargo test --workspace --no-fail-fast` **2856 通过 / 0 失败**（2854＋2 存储故障用例，逐条对账）。

## 7. 剩余风险与登记

- ~~磁盘级 I/O 失败不模拟~~（补验轮已闭环，见 §6a：两用例 I/O 级覆盖，
  实现无缺陷）。
- 结构化输出端点参数（Chat Completions `response_format`：type＝`json_schema`，`name/schema/strict` 位于 `response_format.json_schema` 内）M3 不发送：
  机器级保证在响应侧严格解析（登记差异：违规输出成本一次调用）；供应商
  实测与是否默认启用待 M7。
- `rollout_cwd`＝调用方传入（当前进程目录）：会话工作目录未持久化，登记
  差异（007 meta 仅 `{"v":1}` 不含 cwd）。
- 模板渲染无转义引擎（单遍替换）：会话内容含模板样式文本已测不二次替换。
- threads 登记接线与后台触发属 M7（本轮零触碰）。
