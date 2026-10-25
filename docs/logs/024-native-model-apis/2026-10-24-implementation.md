# 024 原生模型接口实施记录

- 状态：实现中；未验收。
- 规格：[024](../../specs/024-native-model-apis.md)。
- 分支：`feat/024-native-model-apis`。
- 工作树：`/workspace/.worktrees/024-native-model-apis`，从 `ad7959222e7227afecb6811f38ca2b62ea6ef7f2` 建立。
- 原 `/workspace` 的 `feat/023-mcp-resources` 与未提交实现保持原位；只复制已确认的规格、项目规范和相关文档，不复制在途 MCP 代码。
- 构建目录：工作树自己的 `target/`，每条 Cargo 命令显式指定 `--target-dir`，不使用全局共享产物。
- 测试边界沿用规格 §5.2：真实 CLI／PTY、HTTP／SSE 边界服务、会话文件，内部协议转换不 Mock。

## 基线与 TDD

- 原有接口、工具循环和会话相关测试通过：51＋39＋27＋19＝136 项，失败／忽略均为 0，原始输出见 `baseline-tests.log`。
- 第一条新增用例：Responses 文字请求直接使用 `/responses`、`input` 和 `store:false`，保留网关前缀，且只发送一次。
- Responses 文字红绿验证完成：旧实现报“不支持 openai-responses”；同一断言实现后通过，见 `tdd-responses-text-{red,green}.log`。
- Responses 真实 CLI 工具轮红绿验证完成：旧路径拒绝 reasoning Item；实现后工具实际执行一次，原始 `call_id`、独立 Item ID 和完整 reasoning 在下一请求中保持，2 项新增用例通过，见 `tdd-responses-tool-{red,green}.log`。新增共同历史类型导致 TUI 穷尽匹配编译报错，按 diagnosing-bugs 直接编译反馈定位并补齐可见内容投影；记忆过滤也先剥除原生状态。
- V1 → V2 与重启回放红绿完成：先证明旧 V1 被混写；修复后旧文件字节不变，新 V2 带完整状态并可在重启后的下一请求回放。3 个新增用例和 39 个原有会话用例通过。旧“未知版本”负例从 V2 改为 V3，保留原断言意图。
- 一次旧 `session_cli` 回归为 26 通过／1 失败，失败是 `/bin/sh: Cannot fork`；同用例不改断言单线程复验通过。环境只读快照有 16248 个既存僵尸进程，但 cgroup 没有报告 pids／OOM 额度事件，不能据此断定具体上限；最终全量会限制测试并发并再次验证，不能用本次定向通过代替全量。另修正新增夹具曾误用外部工具的 `login` 参数：本项目只支持六字段，实际用 `/bin/sh -c`，无需该参数。
- Messages 红绿验证完成：原实现报协议未支持；实现后原生鉴权／版本头、必需输出上限、thinking 签名回传、工具真实执行和 `tool_use_id` 配对通过。目前新增 4 项通过，见 `tdd-messages-{red,green}.log`。原“未支持协议”负例改用真实未知值 `unknown-native`，不再把已新增的 Messages 当作未支持接口。
- Interactions 第一轮编译未完成：rustc coordinator 报 `failed to spawn work thread`／OS 11 `Resource temporarily unavailable`。这是编译基础设施失败，不能标作功能验证通过。重跑采用 `CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=2`，保留首次日志，未删除或放宽测试断言。
- 2026-10-25 继续：第二次编译在 Cargo jobserver 创建 helper 线程时同样报 OS 11。随后环境恢复（单线程探针成功、进程快照不再含僵尸），未停止其他进程或更改系统限制。重跑时前 5 项通过；新增 generateContent 用例如预期因旧代码不支持协议而失败，见 `tdd-interactions-green-recovered.log`。
- generateContent 红绿完成：真实 CLI 执行命令一次、回传原生 functionResponse、保留 Parts／thoughtSignature；厂商没给调用 ID 时只在宿主内部稳定配对，不生成线上的原生 ID。6 项通过，见 `tdd-generate-green.log`。
- 图片红绿完成：负例证明原生入口会静默丢图并在能力未知时发送；修复后四个新增协议均本地拦截未知能力，并在原生用户消息和工具结果位置保留图片，见 `tdd-images-{red,green}.log`。
- 模型目录红绿完成：原生路径原本被协议门拒绝；修复后按厂商鉴权及游标读取完整分页，Google `models/` 资源名转换明确，不推断图片／工具／参数能力。新增测试累计 8 项通过，见 `tdd-models-{red,green}.log`。
- SSE 实现依据补充：[Google 官方流式说明](https://ai.google.dev/gemini-api/docs/streaming)，2026-10-25 核对。Interactions 用 `event_type`、`step.start/delta/stop` 和 `interaction.completed`；完成事件只带部分 Interaction，须由步骤组装历史。复用仓库已锁定的 `sse-stream 0.2.5`，不自行实现 SSE 行语法。
- 五协议 SSE 文字／工具红绿完成：先证明不支持 SSE 与未组装参数会失败；实现后保留参数／thinking／签名，完成后实际执行一次，中断时零执行，见 `tdd-sse-{text,tools}-{red,green}.log`。
- 来源切换红绿完成：旧实现连已闭合历史也拒绝；修复后更换厂商配置／模型／协议／地址／Key，或切回 Chat Completions，只转移可见历史与明确标注的旧工具内容；未闭合链零发送，见 `tdd-owner-{red,green}.log`。
- 错误上下文红绿完成：结构错误原本缺少来源与请求 ID；修复后错误包含厂商／模型／协议、可用的状态与请求 ID，凭证与指纹不出现在错误中，不自动重发，见 `tdd-error-context-{red,green}.log`。
- 初次记忆与结构化输出覆盖五协议，14 项通过。已核对 phase1 实际是 JSON 提示词＋严格结果解析，不默认添加 `response_format`；显式结构化参数按所选协议原生字段及能力声明传递，未加入多余转换框架。后台 `ModelSend` 同样携带厂商配置身份。
- 全面测试发现公共入口接受错误的 `messages` 类型；修复时进一步发现旧地址校验只检查 scheme。按 diagnosing-bugs 的确定性反例补齐数组／stream 类型与基础地址 query／fragment／内嵌凭证拒绝。统一在 HTTP 前校验，CC 图片能力门在共同历史投影后再次检查。15 项通过，见 `diagnosis-preflight-{red,green}.log`。
- 全面测试 19 项通过：32／33 项、重复 ID、后项缺字段时整批拒绝；未知名称／坏 JSON／业务参数错误逐项失败；合法调用照常执行；第 8 批仍执行并保存；损坏 V2 零发送；模型 ID 及网关编码保持，见 `comprehensive-boundaries.log`。
- 全面测试 22 项通过：五协议请求／响应恰好 10 MiB 与多 1 字节、SSE 累计字节与组装结果分别限额、真实 PTY 屏幕显示与取消后迟到响应零执行，见 `comprehensive-byte-tui.log`。字节期望使用规格的 10 MiB 常量，不从生产上限反算。

## 状态与回归审计

- 状态：本地校验 → 等待 HTTP／收集 SSE → 响应完成 → 整批结构校验 → 串行工具 → 下一请求／终文；保存和恢复保留 assistant 与原生状态同条提交。
- 禁止转换：坏输入到 HTTP、不完整流到工具执行、整批坏结构到部分执行、取消后的迟到响应到执行、缺必要状态或未闭合跨来源链到 HTTP。
- 已有 006／007／016 回归继续保护串行执行、限额、锁与进程会话；新增测试落在公开 API、真实 CLI／PTY、真实记忆文件与本地 HTTP 边界，不 Mock 协议转换。
- 继续审计发现记忆读取器另有 V1-only 头部校验；新增真实 V2 来源测试先复现，再修复，不能用此前的 V1 来源管线通过掩盖此缺口。

## 性能基线

- 脚本：`benchmark.py`。真实行式 CLI（PTY，`TERM=dumb`），通过公开 `/resume` 恢复固定历史；每次完成后再开始下一次，并发 1。独立 mock 进程，不使用真实 Key／用户目录／公网模型。
- 烟测发现 mock 的 Nagle／延迟确认开销约 50 ms；只打开 mock 的 `TCP_NODELAY`，同一短测约 3 ms。烟测不进入阈值数据。
- 正式命令：`python3 docs/logs/024-native-model-apis/benchmark.py --binary /workspace/.worktrees/024-native-model-apis/target/debug/terminal-chat --output docs/logs/024-native-model-apis/performance-baseline.json`，无构建并行执行。
- 30 组（五协议 × 文字／32 调用／准备后图片 × JSON／SSE），每组预热 20 次，200 次 × 3 轮，18,000 个正式样本。每组取最差 p95；范围 3.431–13.431 ms。预热后只重置目标子进程的 VmHWM，再记录 21,924–23,432 KiB 峰值，与同负载 CC 的差为 −184 至 +540 KiB。
- 各组上限为该组基线的 3 倍，已经写入 `performance-baseline.json` 并冻结；原始样本、请求／响应字节、环境／锁文件散列及源文件归档随证据保存。最终版本用 `--baseline` 逐组对照，不能只比较汇总最大值。
- 这属于本地固定响应测量，费用及真实模型耗时仍未测。

## 复用核对

- 已读取固定 Codex Responses 端点代码和 Rig Gemini 转换源码；它们分别使用自己的请求／状态类型，直接引入完整客户端不能自动满足本项目的历史格式、10 MiB 总量及取消／不重试契约。优先复用现有 HTTP 与 SSE 依赖，按官方字段做最小协议适配；具体取舍随探针记录，不宣称候选已全部接通。

## 交付收口（2026-10-25）

接续交接记录完成剩余修复与验证，新增测试累计 30 项全部通过：

- 保存状态校验（上一轮遗留未编译改动）：编译通过，定向与全集复验，见 `native-state-green.log`（24 项）。
- 原生 stream 绕过能力检查（审查 P2-1）：红例 `diagnosis-stream-gate-red.log`（openai-chat 正确拒绝、openai-responses 实际发出请求）；`apply_params` 统一走 `check_request_param`，五协议 × 空列表／不含 stream／能力未知全部零 HTTP。
- Gemini 无参 functionCall 被整批拒绝（P2-2）：红例 `diagnosis-gemini-args-and-interactions-sse-red.log`；官方 `args` 可省略，缺省与 null 按 `{}` 参与，显式非对象仍拒绝（含 null 变体与恶意矩阵），原生 Part 逐字回传。
- 损坏 Interactions SSE 崩溃（P2-3）：同上红例（子进程 panic `cannot access key "status" in JSON string`）；`interaction.created` 先验对象类型，任意畸形形态（字符串／数字／数组／null／乱序）走协议错误，见 `security-hostile-green.log` 与 `security-final-2026-10-25.md`。
- 注释 P3 两处（session.rs meta 门、api.rs post_chat_json 返回值）与 prompt.rs `unused_mut` 一并修正。
- 交接"未证实项"证实并修复：旧 CC 历史非法 JSON 参数对对象参数协议整轮阻断，违反 §1.4；新增 `textualize_unexpressible` 把不可表达工具块（调用＋配对结果）转明确标注历史文本，红例 `diagnosis-legacy-args-red.log`。取舍：同批混合合法／非法参数时整块转文本（保守，不拆分保留合法调用），不伪造目标协议调用块。
- N14 旧版产物拒绝 V2：ad79592 旧版二进制隔离构建后读 V2 夹具明确拒绝（`meta 无效：整文件拒绝`），新程序同夹具恢复 2 条，见 `old-binary-rejects-v2.log`。
- N11 空名单／重复游标／中途失败：`list_models` 共享路径由 002 已验收回归覆盖（`llm_flow.rs` 空名单清空、游标未前进），原生特有字段映射由 `native_model_lists…` 覆盖，不重复造用例。
- N07 组合分解：CLI×五协议图片组装与能力门实测；TUI×原生链路（`tui_renders…`）与 TUI×图片入口（020 已验收）分别覆盖，TUI×原生×图片三重组合未单独实测（正交链路，风险分解见规格 §5.2）。
- N16 总时限：连接 10s／总 180s 沿用 002 已验收语义（`llm_flow.rs` 短超时回归），024 未改时限实现。

## 全量首跑暴露的回归与修复（2026-10-25）

024 以来首次全仓库回归（此前仅有相关子集基线）暴露 4 项失败，红例在 `full-verify-final.log`：

- `session_perf::perf_tiers_baseline_driven`：**真实性能回归**。起点 ad79592 对照通过、
  当前失败（256 MiB 档单条追加 2.1–2.5 ms vs 1 MiB 档 43–45 µs）。根因：`on_history` 每次
  调用执行 `needs_v2` 全量历史扫描，O(n²) 累计。修复：`Session` 增加 `native_seen` 增量
  游标，只扫新增段；resume 四个分支同步初始化为已装载长度。绿例 `diagnosis-regressions-green.log`。
- `context_injection::t15` / `memory_inject::m6`：024 在 body 增加 `_history`（宿主交给适配器
  的共同历史，openai-chat 发送前移除）后，断言按旧请求结构写。修正断言对象为发送面
  （messages／排除 _history 字段），保护意图不变：跳过来源不入请求、记忆读失败不注入。
- `llm_flow::json_error_message_capped_at_300_chars`：024 §1.6 要求错误含厂商／模型／协议与
  HTTP 上下文（实测 395 字符），旧总长上限 340 过时。核心断言（X≤300）不变，总长上限更新
  为 420 并注明契约来源。
- Clippy（此前未运行）：024 引入的警告仅 `native.rs` 一处 if-collapse（已修）；其余 app 层
  警告（catalog／memory.store／tui_host／turn.rs 两处／common/pty.rs 等）经 diff hunk 核对
  均为原有代码，按精准修改原则不触碰。见 `clippy-final.log`、`clippy-final-rerun.log`。
