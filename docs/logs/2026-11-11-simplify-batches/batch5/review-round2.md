# 批 5.2 验收审查第二轮 findings 处置（2026-11-11）

补跑 reviewer（并发限流后串行）：testing 9＋security 2＋maintainability 9＋
performance 2＋reliability 6＝28 findings（全部 low/info——首轮 P1/P2 已在
3036cb3 修复）。处置：

## 修复（代码）

1. **testing medium**：A7 写线程首轮实施漏线格式长度前缀（裸 JSON 致帧流错位
   ——js_orchestration/multi_agent 假红抓出）已在前一提交修正；本轮回补
   shutdown_frame 线格式 roundtrip 锚（protocol tests）。
2. **maintainability medium**：commit_event_line 的 untracked_kind 字符串参量
   → `UntrackedKind` 私有枚举（编译期消除拼错静默丢计数）。
3. **security low**：merge_last None 分支 total_bytes 双计费（1 MiB 累计约 2 倍
   提前触发）——pending 判定收口不再重复计费。
4. **security info**：view_image 沙箱解码后复验 4 MiB（防御深度收口）。
5. **reliability low**：reply_leaf 序列化失败改本地断连收尾（不送 0 长度帧）。
6. **reliability low**：commit_event_line advance=true＋Some 组合 debug_assert
   互斥（防 flush 永久漏写）。
7. **reliability low**：reap_where 双 SIGTERM 冗余删除（helper 统一发送）。
8. **maintainability low×5**：TEXT_CHUNK 旧注释行删、turn.rs 断链文档改
   TurnRequestOptions、utc_basic_from_secs 出处改 007、turn.rs `..Default`
   冗余删、S10 批次 api.rs 空块修复（P1 同轮）。
9. **maintainability low**：store.rs configure_conn/open_existing 文档误挂
   归位＋缩进修复；openai-responses 裸串 12 处 → `PROTOCOL_OPENAI_RESPONSES`
   常量（api/native/stream/context_window）。
10. **testing low**：A8 集成锚 TEXT_CHUNK 注释 512→160 修正。

## 登记不改（依据）

- **testing medium**：普通模式套件未内置 TCHAT_SANDBOX 隔离——结构性既有
  约定（compose 注释+AGENTS 全量验证命令均带 env -u），e2e 双分支为范式；
  改造属独立工作，登记。
- **performance low×2**：S28.2 tail 峰值 <2 MiB（方案已知代价，注释已写明）、
  S28.3 publish 双取锁（operate 互斥排除，良性）。
- **reliability low×3**：shutdown 帧异步入队语义（结果等价，killpg 兜底）、
  S13 反向残余窄窗口（fail-safe 固有形态，自愈）、A7 写线程死亡路径测试
  （端到端大结果用例重——testing_gaps 登记）。
- **maintainability low**：manager 8 处失效行缩进（非语义噪音，与 fmt 缺失
  同源，登记）。

## 验证

- `cargo check --tests`：0 错误；警告 **580＝基线零新增**。
- 全量套件（findings 修复前基线轮）：**55/55 PASS**；修复后受影响套件重跑：
  js_orchestration 45/0、multi_agent 25/0、session lib 31/0、memory lib 49/0、
  proc_session 24/0、context_management 43/0、view_image lib 24/0、
  memory_lifecycle 6/0、mcp_l1 14/0、shutdown 锚 1/0、lib js 15/0。
