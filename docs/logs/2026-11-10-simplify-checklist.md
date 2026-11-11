# 2026-11-10 简化审查清单（ce-simplify-code standalone）

- 来源：用户指令「先检查代码是否需要简化，列出需要简化的清单」；用户选定范围 `src/app/src` 全部产品代码（不含测试）。
- 方法：4 个视角（复用／质量／效率／注释）× 7 块 ＝ 28 个只读审查子代理；全程未运行构建、测试或任何会启动大量进程的命令（容器进程上限）。关键发现由主会话用 grep／读代码二次核实，条目内标注核实状态。
- 原始发现约 190 条，合并同因后：可能缺陷 12 条（A 组）＋ 简化项 37 条（S 组，编号连续跨删除/重构/编码为测试三组）。其余约 110 条低价值发现未逐条列出，需要时可另行导出。
- 证据：`docs/logs/` 各块审查报告的原始输出保留在会话记录中；本清单每条自带位置、问题、改法与依据，可独立执行。

## 使用方式

- 逐项处理：完成一项把 `- [ ]` 改为 `- [x]`，并在该条目下追加一行 `- 处理：<日期> <做了什么/验证结果/跳过原因>`。
- 顺序建议：缺陷 A1–A7（行为问题）→ 删除 S1–S7（零风险）→ 重构 S11 起逐项（每项跑受影响测试）→ 测试 S33–S37（多数先红后绿）。S27（Windows 跨盘，行为变化）与 S31（prompt 参数链，大改）单独确认后再动。
- 全程不动安全检查、信任边界校验、凭证保护；每项须保持输出、错误、副作用与顺序逐字节不变（S27 除外，须登记规格）。
- 状态：**未开始**（本文落盘即第一步完成）。

---

## A. 可能缺陷（12 条，非简化项）

### A1. 多 Agent 组事件双重计费 `- [x]`
- 位置：`src/app/src/agents/runtime.rs:1744-1746`、`:1822-1825`；计费出口 `agents/registry.rs:255-258`。
- 问题：`child_release_reserve(reserved, Some(size))` 经 `commit_event_bytes` 已把真实字节数计入 `event_bytes_committed`，紧接着 `account_event(&event)` 再计一次——每条子记录计费 2×，64 MiB 组预算约 2 倍速消耗（偏保守，无安全风险）。
- 依据：**主会话已核实**（两函数各自对 `event_bytes_committed` 做 saturating_add；注释「审查 N2：不再以 0 冲销」表明意图是计费一次）。其余全部提交点只单次计费。规格 035 §1.6 与模块注释均为计费一次。
- 改法：两处删去 `account_event(&event)`（保留 release），或 release 传 `None` 保留 `account_event`——二者等价；修复后跑 multi_agent 预算相关用例（配 S33 回归锚）。
- 处理：2026-11-10 采纳「删 `account_event` 保留 release」方案（与 `account_event` 文档「预留的释放/冲销由调用方经 commit_event_bytes 完成」一致），两处各删一行并在注释标注 A1 与回归锚名。证据：S33 先红（sum=1390 vs counter=2024）后绿（相等）；`multi_agent` 全套 24 通过/0 失败/3 忽略。详见 `docs/logs/2026-11-10-simplify-a1a2-s33/`（s33-red.log、s33-green.log、multi_agent-full.log）。

### A2. followup 预算满时死锁 `- [x]`
- 位置：`src/app/src/agents/runtime.rs:872-879`（持 `core` 锁的块内调 `rollback_followup`）→ `:961` 再次锁 `core`。
- 问题：std Mutex 不可重入，组消息预算满时对空闲 Agent 调 followup 挂死根线程。spawn 同类分支无此问题。
- 依据：**主会话已核实**（rollback 调用在 `let core = self.core.lock()...` 块内）。
- 改法：预算判定完成后先 drop `core` 守卫再调 `rollback_followup`（或把回滚逻辑改为在已有锁内执行的辅助）。补一条「预算满＋followup」回归用例。
- 处理：2026-11-10 采纳「判定后释锁再回滚」方案（`message_fits` 局部变量＋块外回滚，判定语义与错误路径不变）。新增回归测试 `a2_followup_budget_full_returns_error_not_deadlock`（tests/multi_agent.rs，直驱＋90s 看门狗，4 接收者×16×16 KiB 恰填满 1 MiB 组字节预算）：先红（followup >90s 无返回、死锁线程持有已预留子槽「60s 未排空」）后绿（0.19s 返回 BUDGET_LIMIT）。证据见 `docs/logs/2026-11-10-simplify-a1a2-s33/`（a2-red.log、a2-green.log）。

### A3. apply_patch Move 错误缺双路径 `- [x]`
- 位置：`src/app/src/apply_patch.rs:1717-1719`（变更阶段失败无路径）；另 `:1637-1646`、`:1669-1671` 等只带源路径；多处手写 `format!` 未走 `ctx`/`both`。
- 问题：违反规格 029 §1.4 第 5 条（错误含源、目标两个绝对路径）。`apply_patch.rs:1594-1595` 注释声称「全部出错点经 ctx/both 构造」与事实相反。
- 依据：审查者逐出错点核对＋主会话抽查 `:1717-1719` 确认（「删除前复核移动源失败：源是符号链接（含悬空），拒绝移动」无路径）。
- 改法：先加失败用例（before_unlink 屏障换源为符号链接，断言错误同时含两个绝对路径，今天失败）→ `:1718` 等改用 `ctx(...)` → 测试通过后删注释。
- 处理：2026-11-10 逐处核实后确认 **§1.4-5（变更阶段）违规仅 ：1717-1719 `Ok(None)` 臂一处**——其余 mutating 出错点（创建父目录/创建目标/写入目标/复核 Err/换对象/删源）已走 `both(...)` 或含双路径内联；审查提到的读取阶段三处（:1637-1649 超尺寸/短读）属 `Failure::validation`＝§1.4-2 域（变更未开始），按「只修 §1.4-5」不扩大。修复＝该臂改走 `both("删除前复核移动源失败", &reason)`（与其余 mutating 出错点同一实现点；:1594-1595 注释由此恢复属实，保留未删）。证据：S34 先红（错误无路径）后绿（含源/目标＋阶段）；回归 lib apply_patch 38/0、集成 55/0、沙箱 10/0。见 `docs/logs/2026-11-10-simplify-a3-s34/`。

### A4. apply_patch `lexical_normalize` 丢弃 Windows 盘符 `- [x]`
- 位置：`src/app/src/apply_patch.rs:1783`（`Prefix(_) => {}`）vs `src/app/src/sandbox.rs:2306-2326` `absolute_lexical`（明文保留盘符，文档要求「禁止两处定义分叉」）。
- 问题：跨盘 Move（`C:\x`→`D:\x`）两侧归一后同形，被误判「同一路径」拒绝。安全语义不变（符号链接别名仍由 `create_new` 排他创建兜底）。
- 依据：主会话已对照两函数逐组件分支。
- 改法：与 S27 合并处理（改用 `absolute_lexical`＋规格登记 Windows 行为变化）。
- 处置（2026-11-11，方案 a）：lexical_normalize 分叉副本删除，apply_move 同路径比较改接 sandbox::absolute_lexical 单点（Unix 输入域逐字节等价核实：RootDir 常量同值、Prefix 臂 Unix 不可达；Windows 盘符/UNC 保留、跨盘 Move 误判已修——方向为修正）；cfg(windows) 单测＋unix 归一化单测新增；029 §1.4 末尾登记修订。Windows 未编译验证（无 target），待 038 CI 腿。证据：docs/logs/2026-11-10-simplify-a4-s27/verification.md。

### A5. MCP 工具结果污染检查在序列化后做 `- [x]`
- 位置：`src/app/src/mcp/manager.rs:627-628`（对投影 JSON 字符串 `contains_secret`）；注释 `mcp/call.rs:8-9` 声称「检查发生在投影之前」。
- 问题：敏感值含 `"`、`\` 或控制字符时被 JSON 转义，子串检查漏报。023 §5.1 复审已在资源路径修过同类（`resources.rs:178-187` 按字符串叶子逐个检查），工具结果路径未跟上。
- 依据：主会话已核实顺序（先 `project_result` 后 `contains_secret`）。
- 改法：先加失败用例（`TCHAT_MCP_TOKEN_A` 设为含双引号的值，断言调用失败＋标准客户端错误，参照 `a16_polluted_result_replaced_with_client_error`）→ 修复改为对 `serde_json::to_value(&result)` 逐叶检查（可复用 `value_polluted`）→ 测试通过后删注释。同类缺口：`catalog.rs:139-141` Schema 也扫序列化串。
- 处理：2026-11-10 修复——①`value_polluted` 自 resources.rs 下沉至 secrets.rs（pub，与 `SensitiveValues` 同文件；resources.rs 顶部导入共用，资源路径行为不变）；②call.rs 新增 `project_result_value`（to_value 一次供检查与投影共用）＋`value_to_tool_text` 转 pub，`project_result` 改为委托（call.rs 自测继续使用）；③manager.rs 工具结果路径改为 `project_result_value` → `value_polluted` 逐叶 → 通过才投影文本；is_error 取自同一 Value（原 from_str 回析等价）。call.rs:8-9 注释修复后内容属实，保留未删（与 S35 原文「删注释」偏差已说明）。回归转义敏感值场景：先红（failed=false 直通）后绿（客户端错误「敏感数据」）。**同类缺口 `catalog.rs:139-141`（Schema 扫序列化串）本轮未动，保持登记。**证据见 `docs/logs/2026-11-10-simplify-a5a6-s35/`。

### A6. MCP `env_http_headers` 重复键静默覆盖 `- [x]`
- 位置：`src/app/src/mcp/config.rs:222`（字段值经 `next_value::<serde_json::Value>()`）＋ `:467`（`as_object()` 取头对象）；注释 `:128-129` 声称「各层对象都手动迭代键值对」。
- 问题：serde_json Value 反序列化静默合并重复键，`{"X-A":"A","X-A":"B"}` 被接受（后者覆盖前者），违反 022 §2.1。`:481-488` 只拦大小写不同的重名。
- 依据：主会话已核实解析结构（server 层 visitor 拒重，Value 层不拒）。
- 改法：先在 `tests/mcp_config.rs` `a02_duplicate_json_keys_rejected_at_every_level` 加「头名完全相同」断言（今天失败）→ `env_http_headers` 层改用拒绝重复键的 visitor → 通过后删注释。
- 处理：2026-11-10 修复——`RawServerFields` 增 `env_http_headers: Option<RawHeaderField>`；ServerVisitor 对该键特判，值经 `RawHeaderField`（`deserialize_any`：`visit_map` 在原始条目流上逐键收集并拒绝重复键，错误文案沿用既有「重复配置字段：{key}」；标量/序列/单元降级 `NotObject`）；`parse_http` 增参接收，`NotObject` 按既有文案报「env_http_headers 必须是对象」，对象条目沿用原有校验逐字不变（头名非法/保留头/大小写重复/值非字符串/空白）。config.rs:128-129 注释修复后内容属实（各层确已手动迭代），保留未删（与 S35 原文「删注释」偏差已说明）。先红（解析成功且 `env_http_headers: [("X-A","V2")]` 后值覆盖）后绿（「重复配置字段」拒绝）。**同类缺口：stdio `env_vars` 为数组（无对象内层键），不受影响；其余 Value 值字段无内层对象键。**证据见 `docs/logs/2026-11-10-simplify-a5a6-s35/`。
- 处理（审查轮，2026-11-10）：独立审查（rules/REVIEW.md 新上下文）发现阻塞 P1——`env_http_headers` 移出 entries 后，stdio 跨传输混用检查 `get(field)` 查不到该字段，`command` 服务器携带头配置被静默接受并忽略（认证头丢失不可见，违反 022 §2.1 HTTP 专用与 A02「跨传输混用拒绝」）；另非阻塞 P2——非空数组经 visit_seq 未消费元素，serde_json 在解析层报 trailing characters 而非下游「必须是对象」（fail-closed 保持但文案不指明字段）。修复：①跨传输检查对 `env_http_headers` 改查特判字段 `raw.env_http_headers.is_some()`（错误文案不变）；②`visit_seq` 排空全部元素（IgnoredAny）再降级。回归锚：`a02_cross_transport_fields_rejected` 追加 command＋env_http_headers 用例、`a02_wrong_types_rejected` 追加非空数组用例——先红（stdio 配置解析成功／trailing characters）后绿。mcp 八套件串行重验全绿（config 17、http 15、stdio 36、turn 26、l1 14、resources 37、stdio_sandbox 2、lib 4）。证据 `review-red.log`、`review-green-*.log`。

### A7. JS 编排通道无界 `- [x]`
- 位置：`src/app/src/js_orchestration/runtime.rs:527`（`mpsc::channel` 无界）vs 模块头 `:7-11`（「有界事件通道」「worker 写 stdout 阻塞形成天然背压」）。
- 问题：读线程每帧 `send` 不阻塞，无人观察期间输出帧与 call 帧在宿主内存无上限堆积。规格 034（第 70/92/112 行）要求有界队列、「不留无限积压」。
- 依据：主会话已核实通道类型与读线程。
- 改法：`sync_channel`（带界）或显式背压策略——**行为变更（可能引入读线程阻塞语义），先与用户确认方案**；注释按定案后状态修正。
- 处置（2026-11-11，方案 a，N=16，随定案落地）：通道构造单点 `event_channel()` → `sync_channel(EVENT_CHANNEL_CAPACITY=16)`（积压上界 16 MiB；`CellHandle.event_tx` 改 `SyncSender`）；模块头 ：7-14 按有界事实改写（S8 暂缓项销账）；034 §1.4 末尾登记修订（让出期 16 帧后停等的可观察变化＋三终止路径读线程不悬挂）。send 阻塞缓解选「接受滞留至 runtime drop」：`orch_cell` 为轮函数局部、close 后相邻两行即置 None（turn.rs:1506/1517），三路径（worker 死亡→观察收割腾容量→ReaderDead、墙钟兜底；runtime drop→接收端弃置→send Err；轮终止→close 5 秒有界收割＋弃置）均不悬挂，无死锁（消费侧等待恒有界）。有界性锚测试 `event_channel_is_bounded_at_declared_capacity`（容量＝常量、超容量 try_send 被拒、断连 send 报错；零时序依赖）；「超 N 帧停等」真实 worker 测试需时序断言，不可稳定观测，不硬凑。验证：lib js_orchestration 13/0（含锚）、集成 42/0、security 20/0；check --tests 0 错误、警告 605＝基线零新增；perf 套件 3/7 超冻结阈值为环境漂移存量（干净树对照同失败集合，采样 332.5ms vs 阈值 300ms、基线 13.4ms，toolchain 1.99.0→1.99.1；已于 2026-11-11 重冻结，见 docs/logs/2026-11-11-c18-refreeze/）。证据：docs/logs/2026-11-10-simplify-a7/implementation.md。

### A8. JS 多片输出合并永不发生 `- [ ]`
- 位置：`src/app/src/js_orchestration/worker.rs:299`（所有分片发 `output_frame`，`cont:false` 恒定）；`protocol.rs:75-77` `output_frame_cont` 零调用；宿主 `merge_last`（`runtime.rs:1404-1410`）不可达。
- 问题：034 §1.3「一次 text 原样合并回一条」未实现；另单次 `text()` 含大量控制字符时 JSON 转义膨胀约 6 倍，超过约 17.5 万控制字符即撑爆 1 MiB 帧上限；任何单次超 64 KiB 的 text 被待交付缓冲丢弃。
- 依据：审查者 grep 全仓 `output_frame_cont`／`"cont"` 仅命中定义与恒 false 构造。
- 改法：二选一经用户裁决——(a) worker 首片 `output_frame`、后续片 `output_frame_cont`，使合并语义生效（行为变更，须验证）；(b) 确认多片分开交付可接受，删除 `output_frame_cont`／`merge_last` 死路径（无行为变化）。

### A9. view_image 沙箱图片上限与注释不符 `- [ ]`
- 位置：`src/app/src/view_image.rs:248-249`（注释 4 MiB＋1024 余量）vs `EncodedImage.bytes` 按 JSON 十进制数组编码（约 3.6 倍膨胀，实际有效上限约 1.1 MiB）。
- 问题：031 沙箱模式下 1.1–4 MiB 图片报「超过有界上限」，与进程内路径（4 MiB）不一致。
- 依据：静态推算（审查者），未运行验证。
- 改法：加单测断言「4 MiB 图片编 JSON 后不超过该常量」（今天失败）→ 改传输编码（base64）或按编码后长度定上限；修好后注释属实并保留。

### A10. Windows 三处静态疑点 `- [ ]`
- 位置：`proc_session.rs:36,2221-2222`（`WAIT_FAILED` 未导入成 catch-all 绑定，超时分支不可达）；`proc_session.rs:2391`（Windows 读取线程不累加 `reader_eof`，每会话退出多等 2 秒 COLLECT_GRACE）；`permissions.rs:718-721`（UNC 还原 `format!("\\\\\\{rest}")` 产生三个反斜杠）。
- 问题：注释声称的 Windows 行为与代码不符；CI Windows 为 continue-on-error 且只跑 `cargo check`，无 `-D warnings`，不可达分支告警不拦截。
- 依据：静态阅读，未在 Windows 编译运行（本容器无 Windows 工具链）。
- 改法：修代码＋在 Windows runner 验证（配 S37）；或先在注释/登记本中记录，待 038 Windows 轮处理。

### A11. session auto 文件句柄只写（推断）`- [ ]`
- 位置：`src/app/src/session.rs:1535-1539`、`:1635-1639`（unix 新建 auto 文件未加 `.read(true)`）；`:1543-1545`、`:1643` 注释用「LockFileEx 需要 GENERIC_READ」解释（Windows 已改命名互斥体，前提失效）。
- 问题：同进程对新建 auto 文件执行 `/resume` 可能 EBADF（从只写句柄读）。现有测试只覆盖经 resume 取得的句柄。
- 依据：审查者推断，未运行验证。
- 改法：补用例（新建 auto 后立即 `/resume`）确认；修则加 `.read(true)`（unix 侧行为微变，登记）；两处过时注释随 S1… 处理（此处指 session 注释组）。

### A12. 疑似从未运行的测试 `- [ ]`
- 位置：`src/app/src/tui_host.rs:1946-1951`。
- 问题：两个 `#[test]` 连同 032 文档块堆在 `project_history_items_renders_orchestration_group` 头上，`project_history_items_renders_checkpoint_boundary` 疑似失去 `#[test]`——检查点投影断言可能从未执行。
- 依据：审查者读代码发现，未运行核实。
- 改法：运行核实（`cargo test project_history_items` 列出实际执行的用例）；若确认，恢复 `#[test]` 标注并跑通过。

### A13. resources `_meta` 污染检查扫序列化串（双层防御一致性，无实际漏报）`- [x]`
- 位置：`src/app/src/mcp/resources.rs` `meta_polluted`（`serde_json::to_value(meta)` 后 `contains_secret(&value.to_string())`）。
- 立项假设（2026-11-10，S8 轮新发现）：敏感值含引号/反斜杠时序列化转义掩盖，items 级漏检（A5 同类）。
- 实证修正（2026-11-11）：**无实际漏报**——读路径下游 `finish_success`（:253）与汇总路径（:597）均对完整投影做 `value_polluted` 逐叶检查，转义敏感值的 `_meta` 仍被整份拒绝（双层防御成立）。探针测试实跑绿：`tests/mcp_resources.rs::m2_and_a13_polluted_results_and_escaped_meta_rejected`（夹具场景 `polluted_meta_escaped`，canary 含引号/反斜杠），兼作双层防御回归锚。证据：`docs/logs/2026-11-10-simplify-a13/a13-probe.log`。
- 待用户定案：是否仍把 `meta_polluted` 改为 `value_polluted` 逐叶（纵深防御一致性＋提前失败，可观察行为无差异）；不改则现状双层已安全。来源＝S8 轮新发现。
- 处置（2026-11-11，用户批准口径）：实证无实际漏报（双层防御：items 级串扫＋下游 `finish_success`（:253）/汇总（:597）逐叶兜底）；探针 c3b 落地为回归锚（`m2_and_a13_polluted_results_and_escaped_meta_rejected`＋夹具场景 `polluted_meta_escaped`）；`meta_polluted` 改逐叶属纵深防御增强已登记、不排期；存疑计数对齐（6→10）随本提交。无源码修复。

---

## S. 简化清单（37 条，编号连续：1–10 删除、11–32 重构、33–37 编码为测试）

### 删除

### S1. agents 死常量与过时架构注释 `- [x]`
- 位置：`src/app/src/agents/mod.rs:75-76,88`（三个零引用常量）＋ `:6-10,74-76,87-88`（「有界通道/泵线程/终态通道」架构注释）。
- 依据：常量零引用已 grep 核实；注释与 writer 互斥串行提交的实际机制不符，035 规格已登记「未引入独立泵线程」。
- 改法：删除常量与注释段（`runtime.rs:6-11` 头注释已有正确版本）。
- 处理（2026-11-10）：三常量（`EVENT_CHANNEL_CAPACITY`/`GROUP_CLOSEDOWN_BUDGET`/`GROUP_CLOSE_BUDGET`）连同其文档删除（grep 证据 `s1-grep.txt`：仅定义行，src/docs/specs/plans/.github 零引用）；架构段落失实句（「事件经有界通道交根调度泵线程顺序落盘」）改写为 writer 互斥串行提交的准确表述并指向 runtime.rs 头注释，段落其余属实现说明保留。

### S2. agents 只写状态与死函数 `- [x]`
- 位置：`agents/runtime.rs:1469` `ChildSpec.wall_limit`（只写）、`agents/registry.rs:105,135` `GroupCore.seq`、`agents/mailbox.rs:28,65` `delivered`、`agents/events.rs:260-263` `event_value`（零调用）。
- 依据：审查者全仓 grep（src＋tests）仅命中定义/写入。
- 改法：逐个删除字段/函数及配套初始化、`#[allow]`、孤儿 import。
- 处理（2026-11-10）：四组全删——wall_limit（字段＋`#[allow(dead_code)]`＋:638/:940 两处初始化）、GroupCore.seq（字段＋文档＋:135 初始化；其余 seq 命中均为 AgentEvent 自身真实字段）、Mailbox.delivered（字段＋drain 内自增；abandon 不涉它）、event_value（函数＋`#[allow(dead_code)]`＋文档）＋events.rs 的 `use serde_json::Value` 孤儿导入（删后 grep 仅剩导入行）。grep 证据 `s2-grep.txt`/`s2-grep2.txt`。

### S3. memory/feedback 死分类族 `- [x]`（保留＋注释分支）
- 位置：`src/app/src/memory/feedback.rs:151-193` `MemoriesUsageKind`＋`as_tag`＋`memory_usage_kind_for_path`（含纯死计算 `version_marker` :175-179）。
- 依据：grep 仅命中定义与自测（`feedback_tests.rs:118-145`）。模块头称其为 A28 语义锚。
- 改法：删除（约 45 行产品＋30 行自测）；或经用户确认保留为语义锚并注明无消费方。同步删除对应测试段。
- 处理（2026-11-10，判断依据）：**走「保留＋注释」分支，未删**。核实结果：①规格消费方存在——013 规格 M8「涉及模块」明确把「使用分类」列为交付面（`docs/specs/013-memory-system-v1.md:456`「上游 usage.rs 直接移植：…＋使用分类，A27/A28」）；②文档消费方存在——`docs/records/2026-10-19-013-m8-implementation.md` 两处记载移植与「A28 语义不依赖解析器」的登记；③A28 行为验收（仅工具打开不计数）由 `after_user_turn` 保证、不依赖本分类，且本地无遥测消费方。按清单「规格/文档消费方存在→保留、在注释注明无消费方」执行：模块头新增「消费方状态（2026-11-10 简化清单 S3 核实）」段——生产零调用（唯一使用＝自测）、保留为 013 M8 上游移植语义锚。`version_marker` 死计算随函数保留（注释自述「归一语义保留与上游一致」，删之反而偏离上游锚面）。

### S4. memory/store 死方法 `- [x]`
- 位置：`src/app/src/memory/store.rs:601-604` `enqueue_global_consolidation` pub 方法零调用（实际只走 `_tx`/`_on`）。
- 依据：grep 排除 `_on`/`_tx` 仅命中定义与文档注释。上游移植残留。
- 改法：删除该 pub 方法，保留 `_on`/`_tx`。
- 处理（2026-11-10）：删除（grep 证据 `s3-s7-grep.txt`：调用仅 ：477/:528 的 `_tx` 变体与 ：603 内部 `_on`；docs 命中均为 m1/m5 实施记录的历史过程叙述，描述 SQL 语义而非该 Rust 方法，非消费方）。

### S5. mcp 死 impl `- [x]`
- 位置：`src/app/src/mcp/manager.rs:1029-1035` `From<NameAssignError> for String`＋散落 import。
- 依据：已核实三处调用（`unwrap_or_default`、显式 `match`、测试）均不经它，无 `?`/`into()` 消费点。
- 改法：删除 impl 与 import。
- 处理（2026-11-10）：删除（复审 grep `s3-s7-grep.txt`：NameAssignError 命中＝naming.rs 定义/返回类型、mcp_l1.rs 测试直接 `matches!` 变体、manager.rs 仅本 impl——无 `?`/`into()` 消费点）。

### S6. sandbox seccomp 死桩 `- [x]`
- 位置：`src/app/src/sandbox.rs:1420-1425` 非 x86_64 `build_seccomp_filter() -> Option<Vec<u8>> { None }`。
- 依据：主会话已核实——该 cfg 组合下唯一潜在调用方 `build_seccomp_fd` 恒返回 Err 不调它；x86_64 版被 `build_seccomp_fd` 调用（保留）。桩的文档「返回 Err」与实际 `None` 矛盾。
- 改法：删除桩；非 x86_64 fail-closed 由 `build_seccomp_fd` 恒 Err 承担。落地时以非 x86_64 目标编译确认（mingw 交叉 check 或 CI）。
- 处理（2026-11-10）：删除桩，原位留一段注释说明 fail-closed 由 `build_seccomp_fd` 恒 Err 承担＋删除缘由。**未编译验证该 cfg**（本机 rustup 此前联网装组件失败，无 aarch64 target）——静态核实依据：桩的 cfg 为 `not(all(linux, x86_64))`，该组合下全部引用点（tests.rs:831/848 两测试 x86_64 门控、`build_seccomp_fd` 非 x86_64 变体恒 Err 不调用）均不触达；x86_64 构建不受影响（cfg 互斥）。待 038 CI 的 Windows/macOS 腿或交叉 check 首跑时顺带验证。

### S7. prompt/turn 死转发层 `- [x]`
- 位置：`src/app/src/prompt.rs:763` `build_turn_request_with_input_host`、`:840` `build_turn_request_with_orchestration`（零外部调用者，链内纯透传）；`src/app/src/turn.rs:909-940` `run_user_turn_with_context_and_host`（唯一调用方是 `run_user_turn_with_context`，文档「CLI 仍走此入口」已失实）。
- 依据：grep 含 `src/app/tests` 零命中（包装链内部除外）；生产入口 `main.rs:1115`、`tui_host.rs:781` 走 `_and_caps`。
- 改法：删除三个函数；`_with_tool_search` 尾调用改直连 `_with_agents(...)`；`run_user_turn_with_context` 直调 `..._and_hosts(..., None, None, false, None)`。过时的入口文档句随删。
- 处理（2026-11-10）：三个函数删除；`_with_memory` 尾调用改 `_with_async_host(..., false, false, false, false)`、`_with_tool_search` 尾调用直连 `_with_agents(..., false, false)`（原经 `_with_orchestration(..., false)` 恒等）；`run_user_turn_with_context` 直调 `_and_hosts(..., None, None, false, None)`——各改接点的实参组合与原透传逐值一致（组装结果逐字节不变，由 prompt_assembly/native_model_apis 既有「关闭路径逐字节一致」测试锚定）。失实文档「CLI 仍走 run_user_turn_with_context_and_host（五工具，仅同步）」删除；`_and_hosts` 文档保留 018/019/021 规格 行（属实）。grep 证据 `s3-s7-grep.txt`。

### S8. 与代码事实相反的注释（约 25 条，最高价值注释组）`- [x]`
- 代表位置：`js_orchestration/runtime.rs:7-11`（A7 定案后处理）；`tui_host.rs:6-9`（「API 非流式」失实、Ctrl+C 语义已改 Esc）；`apply_patch.rs:192-193,236-237,416-418`；`main.rs:682-683`（「关闭旧组」无对应代码，实际机制在 `turn.rs:1437/1941`＋`agents/runtime.rs:235-243`）；`proc_session.rs:1794-1796,1831`；`sandbox.rs:475-478`；`session.rs:1543-1545,1643`；`memory/rollout_input.rs:8-10,151-152`；`memory/workspace.rs:193-195`；`memory/phase2.rs:13-15`；`memory/storage_files.rs:56`；`mcp/transport.rs:304-316`、`connection.rs:280-283`（MCP 取消/回收机制描述失实，A 组无对应行为项，删除并登记「MCP 取消不放行」为开放工作）。
- 依据：各审查者逐条对照代码与 git blame。
- 改法：删除失实注释；其中描述今天仍成立约束的片段保留或移至正确位置。**待行为定案的（A7/A8/MCP 取消）随对应缺陷项一并处理，不先删。**
- 处理（2026-11-10，独立子代理执行）：38 条核实——35 条修正（其中第 2/7/16/27/29/30/31/36 条按「保留属实片段、如实改写」处理而非整段删除，理由与摘录见 docs/logs/2026-11-10-simplify-s8/report.md）；1 条核实属实不改（sandbox.rs:13-14 模块头）；1 条跳过（resources.rs:377-383「逐值检查」实扫序列化串——新发现 A5 同类缺陷嫌疑，注释与代码均未动待用户决定）；第 38 条五处经核实属实保留。相邻同因修正 2 处（apply_patch.rs:174-175、prompt.rs:911-912）。两处暂缓均已收尾（2026-11-11）：js_orchestration/runtime.rs:7-11 随 A7 方案 a 按有界事实改写（见 A7 处置）、A4/S27 词法归一化区域失实注释随 A4/S27 删除（见 S27 处置）。代码缺陷嫌疑跳过 6 处待用户判断：resources.rs:377-383（_meta 扫序列化串）、permissions.rs:719-722（UNC 三反斜杠）、context.rs:262-266（Windows raw_errno 同体矛盾）、proc_session.rs:2215-2216（WAIT_FAILED catch-all）、proc_session.rs:2385-2409（reader_eof 不累加）、session.rs:285-286（截断片段无上限检查）。审查代理另登记存疑 10 项（deny 路径非 NotFound 静默跳过、MCP 取消后分离任务仍放行、命名失败错误被吞、HTTP 适配器三处未登记差异、013 规格 :121 免剥离结论过时、catalog.rs:106 文档挂错位置属 S9、session.rs unix 创建路径缺 .read(true)（同名恢复推断 EBADF，未实测）、context_window.rs:627-630 行内注释仍写装载与提交两点、turn.rs:443 未知工具报错文案只列四工具、S8 若干条按保留属实片段改写而非整段删除（计数口径对齐 report.md §5））——详见 report.md 第 3/5 节。验证：cargo check --tests 0 错误、警告 605=基线零新增、lib 单线程 307/0、diff 非注释行零变化断言通过。证据：docs/logs/2026-11-10-simplify-s8/

### S9. 错挂文档（rustdoc 挂到隔壁函数，约 12 条）`- [x]`
- 位置：`sandbox.rs:1462-1469`、`view_image.rs:74`、`proc_session.rs:1675-1683,1025-1026,1829-1830,2389-2390`、`tui_host.rs:857-860,1077-1082`、`api.rs:471-473`、`apply_patch.rs:1770-1771`（删）与 `:1107-1111`（移回 :1123 上方）、`turn.rs:211-214`（删）、`context.rs:85-87`（移到 :114）、`memory/rollout_input.rs:147-152`（移到 :187 上方）。
- 依据：git blame 显示插入新条目后文档未随移。
- 改法：内容属实的整块搬回原函数上方；内容失实的删除（已在 S8 计）。
- 处理（2026-11-11）：搬移 13 处（含 view_image.rs:74、tui_host.rs:857-860 两处按句拆分挂接）＋核销 1 处（apply_patch.rs:1770-1771 已随 A4/S27 删除）＋改判 1 处（turn.rs:211-214 清单标注「删」，核实内容属实且目标 skipped_without_execution 原无文档，按改法通则搬回——偏差依据记录供裁定）。文字零改动（无指代词需微调情形）；行号漂移按内容定位（proc_session 漂移系同日 S8 修正所致）。验证：cargo check --tests 0 错误、警告 605＝基线零新增、非注释行零变化断言＋注释行多重集相等断言通过（净删 1 孤儿空行＝apply_patch.rs 搬移残留清理）、lib 单线程 309/0（沿 S8 口径；默认并行 3 失败为存量——干净树对照同失败集合，隔离运行两树均过）、--test apply_patch 55/0。逐条依据见 docs/logs/2026-11-10-simplify-s9/report.md。

### S10. 低价值注释约 210 条 `- [x]`
- 构成：复述代码约 90 条；分节标题约 50 条；里程碑/任务历史无现存约束约 40 条；无效果语句辩护（`let _` 系列）约 8 条；过时 lint 抑制（`agents/runtime.rs:1449` `message_label`、`turn.rs:2666` 恒不触发的 `too_many_arguments` 等）；无效果排序＋注释（`mcp/secrets.rs:28-30`）。
- 代表：`proc_session.rs:119-121`（mpsc Sender 自 Rust 1.72 起 Sync）、`sandbox.rs:1331-1358`（seccomp 魔数注释可换 `libc::SYS_*` 常量，编号已逐个核对无错号；解释 unshare 原因与跳转逻辑的注释保留）。
- 改法：随其他改动顺带处理，或单独一轮集中清理。**不动**带规格引用与今天仍成立约束的注释（项目规范要求保留）。
- 处理（2026-11-11）：实际处置 **56 处**（纯删 53＋压缩 1＋豁免属性 2，22 个源文件）——agents/runtime.rs `#[allow(dead_code)]`（message_label 有 3 调用点）、turn.rs `#[allow(too_many_arguments)]`（commit_context_switch 恰 7 参恒不触发）按点名删除；proc_session.rs spawner_tx 过时 Mutex-Sync 半句压缩（保留 033 引用与接收端退出条件约束）；secrets.rs 排序辩护注释删除（全消费方顺序无关已核实，排序代码本身不动登记未排期）；其余 49 处为纯分节标题/名称直译/复述/S7 历史叙述（6 组并行探查 47 文件约 5,300 注释行逐行核实＋人工逐条执行）。**与「约 210」差异**：审计期估计无逐条明细，本轮按「规格链接一律保留」严口径实测，各目录候选率 0.2%–2%，存疑约 60 条保守保留未凑数。**上报**：tui_host.rs `renders_checkpoint_boundary` 缺 `#[test]`（属性错挂相邻测试，从未运行）——真实属性错位，超范围未动待裁决；Mutex 移除与无效排序为代码改动登记未排期。验证：非注释行零变化断言通过（豁免 allow×2＋api.rs 行尾注释特判）、check --tests 0 错误、警告 605＝基线零新增（删 allow 无新增）、lib 单线程 309/0、e2e_acceptance 150/0。证据：docs/logs/2026-11-10-simplify-s10/report.md。

### 重构

### S11. session 持久化孪生管线（约 220 行重复）`- [ ]`
- 位置：`session.rs:1071-1293`（commit_checkpoint / commit_orchestration_event / commit_agent_event_root 三胞胎，diff 仅文案＋计数器）＋ `:1515-1600` vs `:1616-1701`（create_auto_file / create_v3_target_with_tail 86 行孪生，仅 meta 版本、tail、错误前缀 3 处差异）。
- 依据：审查者逐段 diff。034/035 各克隆过一次，第四类事件会再克隆。
- 改法：抽私有提交原语（参数＝主语文案、尾行、游标处理、meta 版本、错误前缀），三个 pub 方法薄委托；两个 create_* 合并带参函数。错误文案逐字保留（「自动保存已停用」路径语义不同）。

### S12. agents spawn/followup 启动记账重复（泄槽风险）`- [ ]`
- 位置：`agents/runtime.rs:615-651` vs `:914-948`（35 行：计数→起线程→失败回滚→PERSISTENCE_FAILED）；另 `:277-327` commit_root/commit_child 25 行孪生；`:690-719` vs `:820-855` 入队＋计费 25 行重复。
- 依据：逐行比对，仅 4 处细节差异（agent 名、initial_task、文案、查找键）。槽位/存活计数只改一处即泄漏。
- 改法：提取 `launch_child(spec)` 与共享 helper；槽位释放顺序（live_children→GLOBAL_CHILD_SLOTS→core）不变。

### S13. proc_session 终止收尾骨架两份（已漂移）`- [ ]`
- 位置：`proc_session.rs:296-348` vs `:389-447`。
- 依据：轮询间隔一处 10ms、一处 20ms。
- 改法：提取 `terminate_sessions_bounded(&[Arc<Session>]) -> Vec<id>`；「移除记录＋Err」与「闩锁＋格式化 Err」的后处理留在各自调用方。

### S14. catalog capability 谓词重复 9 函数 `- [ ]`
- 位置：`catalog.rs:856-958`。
- 依据：主会话确认 28 处 `experimental_supported_tools` 引用集中于此。
- 改法：`fn declares_tool(model, name)`；单名开关变一行；`js_orchestration`/`multi_agent` 的「部分缺≠全缺」语义差异保留，不合并。

### S15. mcp 错误映射与初始化重复 `- [ ]`
- 位置：`manager.rs:672-684` vs `resources.rs:639-647`（CallFailure→客户端错误逐字相同）；`manager.rs:524-536` vs `:703-715`（13 行 ServerState 初始化）；`:288-311` 断连标记（既有 `note_connection_down` 未复用）；`:111-153` 两个弱引用解析 match。
- 改法：`CallFailure::to_client_error`（connection.rs 已依赖 secrets 无环）、`fresh_server_states`、复用 `note_connection_down`、`upgraded_manager` helper。

### S16. api 鉴权头/文案/文本化重复 `- [ ]`
- 位置：`api.rs:345-351` vs `:647-650`（鉴权头路由两份，`anthropic-version` 字面量两处）；`:407-411,504-508,569-573,594-597,631-635`「协议未支持」5 处；`api/native.rs:262-276,289-293` vs `:328-340,349-353`（历史工具调用文本化 ×2）。
- 改法：`with_auth_headers`、`unsupported_protocol`、`textualized_calls`/`textualized_result` helper（`historical_text` 已共用）。协议域内 `starts_with("google-")` 与 `matches!` 等价已核实。

### S17. turn 出口三连与入史重复 `- [ ]`
- 位置：`turn.rs` `close_steer_restore`×25、`close_orch!`×28，出口三连约 19 处；assistant 文本入史 5 处逐字重复（`:1889-1896,1920-1927,1957-1964,1984-1991,1997-2004`）。
- 依据：主会话核实计数。**4 处出口顺序不同（:1843、:1859、:1869、:1906 先 close_orch）——入史顺序是可观察行为，必须逐站点保留原顺序。**
- 改法：仿既有 `close_orch!` 加统一出口宏（逐站点按原顺序展开）；提取 `push_assistant_text`/`push_host_notice`。

### S18. UTF-8 边界截断统一 `- [ ]`
- 位置：`proc_session.rs:1635-1656` 私有 `floor_char_boundary`/`ceil_char_boundary`（已有 std 方法，rustc 1.99.1 在用；审查者称 1.91 稳定，落地时编译确认）；agents/events+runtime、js runtime+worker 共 6 处手写回退循环。
- 改法：调用点改 `str::floor_char_boundary`（或共享 `pub(crate)` helper）；各调用点均有 `len > limit` 前置守卫，语义等价已核对。落地前跑一次编译＋相关测试。

### S19. hex 编码 6 处手写 `- [ ]`
- 位置：`catalog.rs:295`、`agents/events.rs:61`、`js runtime.rs:138`、`mcp/naming.rs:51`、`session.rs:871`、`memory/runner.rs:143`。
- 依据：grep `02x` 仅这 6 处；sha2 0.11 无 hex。
- 改法：共享 `hex_encode(&[u8]) -> String`，调用方自定截断（前 16 字节 vs 前 16 字符的差异各自保留）。

### S20. sent_instructions 每请求重算 `- [ ]`
- 位置：`prompt.rs:594-597`（调用点 `:888`，每轮最多 8 次；静态资源 20,903 字节的确定性纯函数）。
- 改法：`static SENT: OnceLock<String>` 首算后 `clone()`；两条 assert 保留在首算内。输出逐字节相同。

### S21. agents_context_message 每轮重复构建 `- [ ]`
- 位置：`tui_host.rs:721-724`（复用 `:621` 已有 `context_sources`）＋ agents 关闭时白做磁盘遍历（3 审查者收敛）；`main.rs:1057-1068` 同款移入 `agents_ready` 分支。
- 依据：`from_env`＋`build_context_message` 含逐级 `symlink_metadata` 找 .git＋读规则文件；产物仅 agents_ready 时消费。纯函数无副作用，移入分支行为不变（跨 UTC 零点时钟读数一致性反而变好）。

### S22. permissions 重复构造 `- [ ]`
- 位置：`permissions.rs:1236-1244` vs `:1260-1266`（trial 构造两次，中间仅只读 confirm）；`:527-550` vs `:561-596`（parse_path_spec/parse_deny_paths 20 行同构）。
- 改法：确认前计算一次 trial 复用；提取 `parse_existing_abs_path(raw, ctx)`（文案逐字保留）。

### S23. sandbox env 白名单与 fd 收口重复 `- [ ]`
- 位置：`sandbox.rs:102-114` vs `:966-978`（仅 HOME 值不同）；seccomp fd 收口接线在 `proc_session.rs:1757-1770`、`view_image.rs:208-224`、`apply_patch.rs:180-189`、`mcp/transport.rs:255-265` 四处重复。
- 改法：`whitelist_env(home)`；`apply_spawn_fd_hygiene(cmd, seccomp)`（tokio 站点可选）。收口语义不削弱。

### S24. view_image 收尾重复 `- [ ]`
- 位置：`view_image.rs:128-137` vs `:259-268`（10 行结果装配逐字相同）。
- 改法：提取 `to_outcome(Result<...>) -> ViewImageOutcome`。

### S25. session utc_basic_now 手写历法 `- [ ]`
- 位置：`session.rs:737-758`；chrono 已是直接依赖（`context.rs:354-357` 同款先例）。
- 改法：改 `chrono::DateTime::<Utc>::from_timestamp(secs,0).format("%Y%m%dT%H%M%S")`；输出逐字节一致（u64 秒域内非 None）；补固定秒数断言（0→`19700101T000000`）；删「无 chrono 依赖」注释。

### S26. memory store/workspace 重复 `- [ ]`
- 位置：`store.rs:182-187` vs `:218-223`（pragma 三连重复，持久化契约单点化；`open_existing` 无 CREATE 的差异保留）；`store.rs:1183-1195` 改 `query_row(...).optional()`（同文件 16 处先例）；`workspace.rs:509-573`（diff 开关向量 3 份＋错误臂 2 份）＋ `:415-424` env 隔离块重复。
- 改法：`configure_conn`、optional() 改写、`diff_args_base()`/`isolate_env()`。

### S27. apply_patch 词法归一化复用（含 Windows 行为修正）`- [x]`
- 位置：`apply_patch.rs:1775-1789` `lexical_normalize` vs `sandbox::absolute_lexical`（`pub(crate)`，`permissions.rs:791` 已复用）。
- 改法：apply_move 改用 `absolute_lexical`；**行为变化**：Windows 跨盘 Move 由误拒变放行（方向为修正），在 029 规格登记；符号链接别名仍由 `create_new` 排他创建兜底。单用户确认后执行。
- 处置（2026-11-11，随 A4 方案 a 落地）：lexical_normalize 已删，单一实现点达成；sandbox 侧文档补 029 消费方与分叉史；S8 暂缓的区域内失实注释（错挂「末端符号链接拒绝」残留文档）一并删除。验证同 A4 处置行。

### S28. 效率三强项 `- [ ]`
- `api.rs:540-549`：每请求整体深拷贝 request（messages 即被覆盖）＋ `_history` 二次克隆 → 选择性克隆＋一次移交给 `from_value`；`:547` `json!(messages)` → `Value::Array(messages)` 直移。
- `proc_session.rs:1591-1613`：`PendingBuffer` 超 1 MiB 后每 8KiB 追加全量 memmove（大输出热路径 O(n²)，022 同类坑）→ 前端偏移缓冲或阈值批量收缩（`take()` 输出逐字节不变）。
- `mcp/manager.rs:795-905`：每轮无条件重建快照（每工具 3 次 Schema 深克隆）→ 变更点清缓存的 `Arc<McpSnapshot>` 复用（快照不可变，Arc 身份不可观察；每轮仍重查断连原子量）。

### S29. 效率次强项 `- [ ]`
- `tool_search.rs:392-400` 序列化回析（Value 恒等）＋ `:434-472` `build_search_text` 每条目两遍；`agents/runtime.rs:1613` 子轮上下文整体深克隆（锁内构建即可）；`js runtime.rs:452-458` init 帧序列化两遍（上限 10 MiB）；`agents/runtime.rs:285-335` 事件每次提交 2-3 次完整序列化＋克隆入队（与 S12/A1 联动：commit 内序列化一次返回字节数）。
- 改法：各自按条目改；输出与错误逐字节不变。

### S30. 裸串与同体 helper `- [ ]`
- 位置：`tui_host.rs:541,577,1303,1704`＋`main.rs:843,856` 裸串 `"send_message_to_user_async"` → `message_to_user::TOOL_NAME`；`catalog.rs:105-107,144-146` 同体 serde helper → `BTreeMap::is_empty`；`main.rs:318,361,467` y/yes 谓词 ×3 → `is_affirmative`；`tui_host.rs:911-913` vs `main.rs:592-594` 未知命令提示串共享常量。
- 改法：纯等价替换。

### S31. prompt 参数膨胀链收敛 `- [ ]`
- 位置：`prompt.rs:751-939`（7 层包装、终端函数 13 参数需 allow；S7 先删两层）。
- 改法：能力布尔与 tool_search/mcp_tools 收进 `Default` 开关结构体，单一组装入口；4 个测试文件机械更新。**改动面大，单独一轮并经用户确认。**

### S32. 跨模块符号统一 `- [ ]`
- `agents/runtime.rs:1959-1967` `with_native` 逐字副本 → 复用 `turn.rs:25-32`（提 `pub(crate)`）；`agents/events.rs:206-211` 状态名词表 → `AgentState` 单一来源（`from_display` 或常量数组，V3 装载校验防漂移）；`api.rs` 四协议裸字符串 42 处 → 常量（拼写错误静默落 `_` 臂的现状消除）；`turn.rs:267-288,2010-2019` `ParsedToolCall` → `pub use prompt::ToolCallRecord` 别名（恒等映射搬移消除）。

### 编码为测试（先写测试，通过后删对应注释；多数今天失败＝A 组回归锚）

### S33. 组事件计费一致性 `- [x]`
- 断言：`event_bytes_committed == Σ 已提交事件 event_bytes`。**今天失败**（A1）。A1 修复后此测试转绿，随后删除 `agents/runtime.rs:1743`「审查 N2」矛盾注释。
- 处理：2026-11-10 落地为 `tests/multi_agent.rs::s33_group_event_billing_charged_exactly_once`（直驱 `execute_control_tool`：spawn→子带工具调用响应→wait→close；断言 Σ已提交非 delivered 事件==计数器，覆盖响应记录与工具结果记录两个双计费点）。配套在 `GroupRuntime` 新增观察面 `event_bytes_committed()`（不改计费行为）。注意：纯文本终文不经 child_record（终文门 Complete 分支直接返回），子脚本必须含工具调用响应才能踩中 A1。先红（1390 vs 2024）后绿；「审查 N2」注释已随 A1 改写为指向本回归锚，未删（现内容属实）。证据见 `docs/logs/2026-11-10-simplify-a1a2-s33/`。

### S34. apply_patch Move 错误双路径 `- [x]`
- 用例：before_unlink 屏障换源为符号链接，断言错误同时含两个绝对路径。**今天失败**（A3）。修复后删 `apply_patch.rs:1594-1595` 注释。
- 处理：2026-11-10 落地为 `apply_patch::tests::move_symlink_swap_error_contains_both_paths`（仿 `move_source_swap_detected_before_unlink` 的接缝子进程机制；替换对象＝悬空末端符号链接，另断言 state_known=false、错误含「删除前复核」阶段、目标保留不回滚、替换对象不被删）。先红（错误为「…源是符号链接（含悬空），拒绝移动」无任何路径）后绿。`apply_patch.rs:1594-1595` 注释经 A3 修复后内容恢复属实，按其现状保留（未删）。证据见 `docs/logs/2026-11-10-simplify-a3-s34/`。

### S35. MCP 污染与重复键 `- [x]`
- 两例：含 `"`/`\` 敏感值的工具结果被整份替换为客户端错误（**预计今天失败**，A5）；`env_http_headers` 头名完全相同的重复键被拒绝（**今天失败**，A6）。修复后分别删 `call.rs:8-9`、`config.rs:128-129` 注释。
- 处理：2026-11-10 落地两例——①`tests/mcp_stdio.rs::a16_polluted_escaped_result_rejected_by_leaf_check`（夹具新增 `polluted_result_escaped` 场景回显 `TCHAT_MCP_TOKEN_Q`＝`canary"q\tail`，断言 failed＋标准客户端错误＋canary 不入投影）；②`tests/mcp_config.rs::a02_duplicate_json_keys_rejected_at_every_level` 追加第三段断言（url＋`env_http_headers` 对象内完全同名的重复头）。先红（转义值漏报直通；`[("X-A","V2")]` 静默覆盖）后绿。两条注释（call.rs:8-9、config.rs:128-129）修复后内容属实，保留未删——偏差与理由见 A5/A6 处理记录。证据见 `docs/logs/2026-11-10-simplify-a5a6-s35/`（红绿×4＋8 套件回归日志）。
- 处理（审查轮，2026-11-10）：独立审查确认 A5 逐叶覆盖与等价性成立（含 is_error 直取与 to_value 失败路径逐字一致）；两条保留注释经代码事实核实成立。**口径内残差登记**（审查 P3，非阻塞）：逐叶检查不再命中「敏感值横跨叶子边界/恰为键名」的巧合结构（旧整串扫描可命中，多为假阳性过拒；023 §5.1.4 已裁定的逐字段口径即逐叶，资源路径同性质已接受）——如需补齐可叠加原序列化串兜底检查（并集），是否采纳由用户决定。`resources.rs:379-381` 对 `_meta` 的串扫经核实均有投影层逐叶兜底，无实际缺口（审查者核实）。

### S36. 审查标签规则补锚（7 条）`- [ ]`
- JS seq 连续无空洞（P1-3，`tests/js_orchestration.rs:1179` 附近）；二次 observe 不再截断（P2-8）；followup 满槽保状态文案（N3）；`proc_session.rs:142-143` reap_failed 清除时机；`sandbox.rs:891-892` 成对断言在组装层强制。通过后删除对应标签注释。

### S37. Windows 夹具回归锚 `- [ ]`
- 三件（配 A10）：WAIT_TIMEOUT 分支可达；Windows 读取线程累加 `reader_eof`；UNC 还原两反斜杠。需 Windows runner（038 工作流）执行。

---

## 未逐条列入的低价值发现（约 110 条）

参数解析辅助家族统一（错误文案各异）、`spawn_piped/spawn_pty` 平台分支样板、js args 裸字面量常量化、`naming.rs` 三处克隆、`Option<String>` 恒 Some 收窄（`Observation.cell_id`）、`memory` pub 收窄 2 处、`Phase.clone()` 可去（Copy 类型）等。需要完整清单另行导出。

## 处理日志

（每完成一项在此追加：`<编号> <日期> <结果>`；全部完成后按 AGENTS.md 全量验证并报告通过/失败/跳过数量。）
