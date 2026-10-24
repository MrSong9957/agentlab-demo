# 022 MCP 外部工具实施记录

- 日期：2026-10-24。
- 分支：`feat/022-mcp-tools`（基线 `f662cf6e4a7881923487ac40b395a20efdb77c31`，合并目标本地 main）；实现全部为工作区未提交差异，等待批准提交。
- 交付：[022 规格](../specs/022-mcp-tools.md) 全量实现；协议层复用官方 `rmcp` SDK 固定 rev `98a54789b80110ad210495c4ad845c89e1980471`（3.5.1，Apache-2.0），`Cargo.lock` 锁定全部传递依赖。
- 安全门禁报告：[2026-10-24-022-mcp-tools-security](../security/reports/2026-10-24-022-mcp-tools-security.md)（17 项攻击面实测＋15 项变体验证）。

## 1. 实现结构（022 §3.2 落点）

| 模块 | 责任 |
|---|---|
| `src/app/src/mcp/config.rs` | mcp.json 严格解析：手写 serde visitor 拒绝重复键（serde_json Map 会静默合并）；未知字段、传输互斥、URL/header/超时校验；停用条目不解析凭证 |
| `secrets.rs` | 每连接固定敏感值集合（Bearer 值＋认证头形式＋显式引用环境值）；`contains_secret` 先于投影／展示／日志 |
| `naming.rs` | `mcp__<server>__<tool>` 净化＋sha2 12 位短哈希＋64 上限＋冲突显式拒绝（移植 Codex `tools.rs` 适配 CC） |
| `catalog.rs` | 原始名去重→Schema 校验（根 type／properties 缺省补空对象）→污染检查→名单筛选→CC 定义（含 description） |
| `transport.rs` | 有界 stdio 传输（8 MiB 行界限，移植 Codex `bounded_stdio_transport`）、进程组回收（`process_group(0)`＋SIGKILL 组）、3 秒有界关闭、stderr 32 KiB 尾部、环境白名单（移植 Codex `utils.rs` Unix 名单）；有界 Streamable HTTP 客户端（移植 SDK reqwest 适配器＋SSE 单事件限额器，差异仅 JSON 体 8 MiB 有界读取；重定向禁用、`reinit_on_expired_session=false`） |
| `connection.rs` | Initialize 生命周期（显式 `LATEST_WITH_INITIALIZE`=2025-11-25；自定义 handler 覆盖 get_info——默认构造会发 `LATEST`，规格 §4 预警的坑，实测踩中一次）；roots/elicitation 反向请求 method-not-found；完整分页（100 页／2048 项／64 KiB 游标／重复拒绝／总时限）；单次可取消 `tools/call`（本地等待侧绑定取消标志＋超时，通知任务 ≤1s，迟到结果随句柄丢弃） |
| `manager.rs` | 管理器：配置加载、连接复用、目录过期计数（stale_epoch，发布基准在列出前记录——重列期间／之后的新通知保留给下一轮）、每轮快照（全局命名）、reload（先解析后替换；operate 互斥）、`/mcp` 状态、退出有界关闭 |
| `call.rs` | 结果投影（保留 content 顺序／structuredContent 显式 null／isError 缺省不补 false／`_meta`）与标准客户端错误形态 |
| `turn.rs` 集成 | `TurnExtras.mcp`＝每轮冻结快照；MCP 定义追加在原生工具后；快照精确命中分派；128 总量上限在发送前按组装结果检查；MemoryMerge 强制忽略 |
| `main.rs`／`tui_host.rs` | CLI：SIGINT 提前接管→prepare（取消＝结束本轮零模型请求）→extras；`/mcp [reload]`；退出 `mcp.close()`。TUI：`run_turn_inner` 内 prepare（取消＝SubmissionRejected）；`/mcp`（reload 经独立线程桥接，busy 由接续槽位判定）；退出 `host.close_mcp()` |

## 2. 函数级移植登记

| 本地文件 | 上游来源（固定提交／rev） | 许可 | 说明 |
|---|---|---|---|
| `mcp/transport.rs` `BoundedStdioTransport`／`StdioProcess` 关闭语义 | `openai/codex@d138398` `rmcp-client/src/bounded_stdio_transport.rs`、`local_stdio_transport.rs` | MIT/Apache-2.0 | 8 MiB 行界限；去 memchr（`position`）与 tracing；补进程组与 stderr 排空 |
| `mcp/transport.rs` `build_child_env` 白名单 | 同上 `rmcp-client/src/utils.rs` | MIT/Apache-2.0 | Unix 缺省名单＋显式引用；去非 inheritable 过滤与 CA 特例 |
| `mcp/transport.rs` `BoundedHttpClient`＋`SseEventSizeLimiter` | `modelcontextprotocol/rust-sdk@98a5478` `transport/common/reqwest/streamable_http_client.rs`、`client_side_sse.rs` | Apache-2.0 | SDK 适配器与限额器为 `pub(crate)` 无法直接复用；差异＝有界 JSON 体读取；去 legacy_discover 分支（本项目不用 Discover 生命周期） |
| `mcp/connection.rs` `list_all_tools` 分页边界 | `openai/codex@d138398` `codex-mcp/src/pagination.rs` | MIT/Apache-2.0 | 100 页／2048 项／64 KiB 游标／重复拒绝；anyhow→本地错误 |

## 3. 关键实现判断（预测→实测差异）

1. **SDK `get_info` 默认发 `LATEST`**：规格 §4 已预警；自定义 handler 显式 `with_protocol_version(LATEST_WITH_INITIALIZE)` 后全部连接正常（实测曾有连接全部报「不兼容协议版本 2026-07-28」，即遗漏该覆盖所致——修复后 A05 全绿）。
2. **重列期间通知保留**：初版「列出后 settle 50ms 再读计数」会把重列期间的新通知当作已处理内容发布（吞通知，违反 §2.2）；修正为**列出前**记录基准计数——期间与之后的通知必然大于基准，下一轮重列（保守多列、不吞通知）。实测 `a13_notification_during_relist_keeps_stale_marker`。
3. **input_required 的 SDK 反序列化形态**：该 rev 将 `resultType:"input_required"` 反序列化为 `ServerResult::CustomResult` 而非 `InputRequiredResult`（分派层 `other` 分支承接）；两个分支都按「不支持的协议响应」失败，变异 M12/M12b 分别检出。
4. **跨服务器命名**：审查发现按服务器分别命名时跨服务器同名不消解；改为 `publish_snapshot` 聚合后全局 `assign_model_names`（同一确定性算法，冲突经哈希消解）；`/mcp` 状态按单服务器本地视角显示，全局冲突时实际发布名附哈希。
5. **取消保留健康连接**：审查发现重列复用的既有连接被误记为「本轮候选」而被取消回收；改为仅 `work == 1`（新建）记录候选。
6. **启动总时限**：审查发现 connect 与 list 各自包一层 timeout（总计 2 倍）；改为单一外层 timeout 覆盖两段。
7. **工具描述进声明**：审查发现 description 做了污染检查但未写入 CC 定义；`ToolEntry.description` 补齐（A06/A08 声明黄金值同步）。

## 4. 测试与验证

| 层级 | 文件 | 数量 |
|---|---|---|
| L1 | `tests/mcp_config.rs`、`tests/mcp_l1.rs` | 17＋14 |
| L2（stdio） | `tests/mcp_stdio.rs`（夹具 `tests/fixtures/mcp_stdio_server.py`，固定仓库测试程序） | 31 |
| L2（HTTP） | `tests/mcp_http.rs`（进程内最小 HTTP/1.1 服务；含 A18 在途退出闸门测试） | 11 |
| L2（turn） | `tests/mcp_turn.rs`（真实工具循环＋stdio 夹具＋mock 模型；含 A17 混合批） | 13 |
| L3（PTY） | `tests/e2e_acceptance.rs` s102／s103／s104 | 3 |
| 单元 | `mcp/call.rs` 内嵌 | 3 |

- 变体验证（15 项，隔离副本可逆）：遗漏实际调用、错误身份映射、绕过名单、绕过 MemoryMerge、isError 画成功、只读第一页、漏判目录过期、404 隐式多发、取消后迟到入史、污染结果透传、stdio 限额失效、HTTP 体限额失效、非 complete 当成功、Schema 校验跳过、重复原名覆盖——全部检出；还原字节一致后用例恢复通过。
- 安全门禁：见报告（R1–R21 审查矩阵＋T1–T17 实测索引）；无高风险项。
- 双轴 code-review：Standards 3 硬违规（未用导入／close_mcp 未接线／注释与行为不符）＋Spec 6 缺陷（TUI 退出清理、跨服务器命名、启动双时限、description 缺失、取消误回收健康连接、污染文案）全部修复并复验；判断题（重复 helper 收拢、native_tool_count 删除、不可达检查删除、私有化、导入上移、文案统一）一并处理。

## 5. 已知缺口与未运行项

- **Windows**：ConPTY、进程对象句柄、Windows 环境白名单（`WINDOWS_CORE_ENV_VARS`）未运行——本环境为 Linux 容器，规格允许列缺。
- **A17 补证（2026-10-24 第二轮）**：MCP 等待时与 019 附图／021 接续的共存路径现在有直接 L2 用例 `mcp_turn::a17_mcp_with_view_image_batch_pairs_then_image_projection`（MCP＋view_image 同批＋批中接续提交：断言 assistant→i1→i2 严格配对、图片附件在完整配对之后（批末投影）、接续输入在其后按来源顺序恰一条入史、服务器账本恰 2 次原始调用）。共用路径引用：投影＝`prompt.rs::project_history` 的批末 `pending_attachments`（019 §4 适配，MCP 结果无 image 载荷不触发）；接续＝`turn.rs` 批末 `channel.drain()`→`ingest_steer_input`（021 §2.3-4，位于完整工具配对之后、下一请求组装之前）；问询配对＝`turn.rs` 批末检查点（018 §8.4，`mcp_turn::a17` 覆盖）。既有 019/021 契约测试（view_image e2e 批序、message_to_user mixed_batch）继续覆盖各自投影边界，不重复。
- **10 MiB 模型请求边界**：由既有 `api.rs` 计量承担（MCP 定义计入 body 后同样受限），未新增专项测试。
- **HTTP 在途时退出（补证 2026-10-24 第二轮，已直接实测）**：`mcp_http::a18_http_inflight_close_bounded_late_result_discarded`——受控闸门确认真实 tools/call 已到达（在途证据）→宿主取消传播→调用返回「已发未知」（不宣称远端未执行）→生产关停入口 `manager.close()` 有界完成（<5s）→释放闸门后迟到结果不进入任何返回值/历史→服务器账本恰 1 次调用（无重放）、close 后 prepare 零发布。修复一处缺陷：close() 原未阻止下一轮自动重连（RED：close 后 prepare 发布 1 工具），现置 listing=None＋error 标记（reload 可恢复）并持 operate 锁（消除与 prepare/reload 的关停竞态）。stdio 维度退出回收由 e2e s103 覆盖（PID 消失实测）。
- **内存放大（低风险，措辞澄清）**：10 MiB 限制只约束**出站模型请求**（完整序列化后计量，api.rs 既有契约），**不构成目录累计内存上限**——单工具 Schema 在接收侧受 8 MiB 消息上限约束，128 项理论上限的目录驻留内存可到 GiB 量级（当前无目录级累计上限，属既有约束，未新增资源预算）。首版接受；后续可按需增加单项 Schema 尺寸或目录累计上限（契约变更需另立规格）。
- lint：`cargo clippy`／`cargo fmt` 工具链未安装（AGENTS §7 既有事实），未运行；`cargo check` 无新增警告（`prompt.rs:190` 的 `unused_mut` 为 019 既有）。

## 6. 验证入口与统计

最终验证（§7 全量）：`cargo check --locked --offline && cargo test --workspace --locked --offline --no-fail-fast`，工作目录 `/workspace`。实施过程中的分项统计见上文各表；最终逐目标统计以最终验证输出为准（见交付报告）。
