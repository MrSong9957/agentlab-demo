# S9 错挂文档修正处理记录（2026-11-11）

性质：仅移动注释挂接位置，行为逐字节不变。清单 S9（约 12 条）逐条处置如下。
注：清单行号为 2026-11-10 记录；proc_session.rs 行号漂移系同日 S8 批修正该文件
注释所致，全部按内容定位核实。文字内容零改动（无指代词需微调的情形）。

## 逐条处置（原位置 → 目标 ＋ 依据）

1. **sandbox.rs:1462-1469 → `confirm_sandbox_with_release` 上方**。031 逐启动
   PID 命名空间验证块原与 038 gate 命令文档连排挂到 `exec_gate_argv`；块内
   「children 文件找回收器／有界轮询／LeaderGone 返回 Ok／超时 killpg 返回
   Err」全部为确认循环体（:1577 起）的实现事实。038 两句留在 exec_gate_argv。
2. **view_image.rs:74 → `unsupported_outcome` 上方（按句拆分）**。首句「能力
   关闭（text-only／未知）时的结果」描述能力未确认分支（调用点 turn.rs:2212
   else 支）；次句「以错误摘要构造结果（031 编码失败等调用方侧失败）」留
   `error_outcome`（调用点 turn.rs:2204＝策略编码失败）。两函数原一有一无。
3. **proc_session.rs:1022-1023（清单记 ：1025-1026）→ `kill_group_checked`
   上方**。「终止进程组的受检入口…测试注入仅对 TEST_KILLPG_FAIL_PGD 生效」
   ——注入点 ：1088 在 kill_group_checked 函数体内（kill_group_sigkill 为
   被其调用的原始 killpg 入口，无注入）。组硬终止句（1024-1025）留
   kill_group_sigkill。
4. **proc_session.rs:1673-1682（清单记 ：1675-1683）→ `tool_argv` 上方**。
   033/031/038 三段描述「构造 exec_command 启动命令」的函数（「本函数内联
   完成 spawn 前后的全部时序」），原整块挂在输出形态 `struct ToolArgv` 上；
   结构体字段级 /// 保留不动。
5. **proc_session.rs:1824-1825（清单记 ：1829-1830）→ unix `spawn_pty` 上方**。
   「PTY 模式：分配真实伪终端…pre_exec setsid」描述 unix 版；Windows ConPTY
   段留 windows fn（两 fn 相邻，unix 版原无文档）。
6. **proc_session.rs:2384（清单记 ：2389-2390）→ unix `spawn_reader` 上方**。
   「readers_may_stop 置位后退出（宽限…不无限等待）」描述 poll 版 unix 读取
   线程；「Windows 读取线程：阻塞 Read 直到 EOF…无需 poll 中断」留 windows fn。
7. **tui_host.rs:858-859（清单 ：857-860 前半）→ `run_command` 上方**。斜杠
   命令两句描述 TUI 分发器（/exit 驱动内建、其余与行式路径同一语义），原挂
   `cmd_mcp`；cmd_mcp 仅保留 022 /mcp 段。
8. **tui_host.rs:860-861（清单 ：857-860 后半）→ `cmd_resume` 上方**。
   「/resume 成功时返回 CommandOutcome::ReplaceCells——驱动据此渲染」描述
   cmd_resume 返回契约（run_command 仅转发），该函数原无文档。
9. **tui_host.rs:1077-1082 → `project_history_items` 上方**。012 历史投影＋
   018-B 合并句描述投影函数（「经与实时 ToolEnd 同一转换 tool_end_item」），
   原挂在 `struct TuiCheckpointSink` 上；032 检查点宿主段留结构体。
10. **api.rs:471-473 → `chat_with_body` 上方**。「协议检查、鉴权、重定向拒绝
    与错误映射与 `chat` 完全一致；`chat` 原行为不变」为 006 拆分时 chat_with_body
    的契约；032 插入的包装 `chat_with_provider_body`（仅注入 `_provider_id` 后
    转发）截留了文档。包装现无文档（原本也无其它文档，行为句「附上 model 与
    request_params」由 build_chat_body 完成，属 with_body 链路事实）。
11. **apply_patch.rs:1107-1111 → `read_bounded` 上方**。「有界源读入核心（fd 级
    fstat 预检＋字节门＋尺寸核对）」描述 read_bounded，原挂在相邻的
    `enum BoundedReadError` 上（文档与枚举间夹一空行）。搬移后清理该孤儿空行
    1 个（授权范围内的空行调整；净删空行 =1，机器断言已定位）。
12. **apply_patch.rs:1770-1771（删）——已随 A4/S27 批解决，核销**。错挂孤儿
    文档（「末端符号链接拒绝的错误判定」）已于 A4/S27 删除（见 S27 处置与
    docs/logs/2026-11-10-simplify-a4-s27/verification.md）；核实现址仅剩删除
    残留的两个空行（纯格式），不重复处理。
13. **turn.rs:211-214 → `skipped_without_execution` 上方（清单标注「删」，核实
    后改判搬回）**。核实内容属实：分工具结果结构（exec_command/write_stdin＝
    ExecResult 序列化、apply_patch＝ApplyPatchResult、未知工具＝单 error 通用
    结构）与函数体逐条吻合，目标函数原无文档——按改法通则「内容属实的整块
    搬回原函数上方」执行，与位置清单的（删）标注不一致，依据在此记录供验收
    裁定。215-216 常量句留 SKIPPED_BY_CANCEL_MESSAGE。
14. **context.rs:85-87 → `load_source_text` 上方**。「有界读取并解码…非法
    UTF-8 或读取失败则跳过并记原因」描述 load_source_text（原无文档）；macOS
    系统前缀重解段（88-93）留 resolve_darwin_system_prefix。
15. **rollout_input.rs:147-152 → `serialize_filtered_history_items` 上方**。
    「序列化过滤后的提炼输入…预算口径差异登记备查」描述该函数（原无文档）；
    032/034 工具名段留 is_context_control_tool。

合计：搬移 13 处（含 2 处按句拆分）、核销 1 处（A4/S27 已删）、改判搬回 1 处
（turn.rs，见第 13 条）。

## 验证日志

- `cargo check --tests`：0 错误；警告 605＝基线零新增。
- 非注释行零变化断言：**通过**——9 个改动文件的 diff 增删行全部为注释行或
  空行（机器断言，见下）。
- 注释行多重集相等断言：**通过**——删除集＝新增集（纯移动、零文字变化）；
  唯一差异＝净删 1 个空行（apply_patch.rs 孤儿空行，第 11 条授权范围内调整）。
- lib 套件（单线程，沿 S8 轮口径）：**309 passed / 0 failed / 1 ignored**。
- lib 套件（默认并行）：306/3 失败（turn::tests::tool_call_id_and_name_preserved_verbatim、
  tool_events_pair_begin_end_and_carry_display、tool_call_id_uniqueness_uses_original_values）
  ——干净树（stash 后 HEAD）对照全量运行**失败集合与计数完全一致**（306/3），
  三者隔离运行在两棵树上均通过——存量并行负载问题（与 S8 轮「lib 单线程」
  口径的由来一致），与 S9 注释搬移无关（零代码变化已由断言证明）。
- `cargo test --test apply_patch`：**55 passed / 0 failed**。

## 机器断言命令与输出

```
git diff -U0 -- <9 文件> > /tmp/s9.diff
# 断言 1：全部 +/- 行为注释或空行 → "ASSERT OK: 非注释行零变化"
# 断言 2：注释行多重集 删除集==新增集 → 通过；净删空行 1（apply_patch.rs）
```
