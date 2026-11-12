# S10 低价值注释清理处理记录（2026-11-11）

性质：只删/压缩低价值注释＋2 处清单点名过时属性；行为逐字节不变。清单 S10
记「约 210 条」为审计期估计值（无逐条明细落盘）；本轮按任务给定保留规则全库
逐行实测，实际处置 **56 处**，差异说明见下。

## 分批处置摘要（7 批，6 组并行探查＋逐条人工核实后执行）

| 批 | 文件 | 处置 | 类型 |
|---|---|---|---|
| 机械/点名 | agents/runtime.rs:1459 | 删 `#[allow(dead_code)]`（message_label 有 3 处调用 :1548/:1700/:1855，属性过时） | 豁免属性① |
| 机械/点名 | turn.rs:2636 | 删 `#[allow(clippy::too_many_arguments)]`（commit_context_switch 恰 7 参，lint 阈值 8+，结构性恒不触发） | 豁免属性② |
| 机械/点名 | proc_session.rs:119-121 | **压缩**：删「Mutex 仅弥补 std mpsc Sender 非 Sync」过时半句（Sender 自 1.72 起 Sync），保留 033 引用与接收端退出条件约束 | 压缩 |
| 机械/点名 | mcp/secrets.rs:28-29 | 删排序辩护注释 2 行（contains_secret/contains_secret_bytes/value_polluted/sanitize_detail 全部为顺序无关布尔判定，values 私有无逐值替换消费方——排序代码本身无效但属代码改动，不动，登记未排期） | 删 |
| memory+其余组（25 文件/约 1172 注释行） | memory/workspace.rs:77、display.rs:159 | 删复述常量注释、纯分节标题（含日期历史标记） | 删×2 |
| core-turn 组（7 文件/951 注释行） | prompt.rs:759-760、812-813、turn.rs:894-895 | 删「S7 已删中间层」历史叙述＋实参复述 3 处 | 删×3 |
| agents+js 组（11 文件/733 注释行） | protocol.rs:35/60/85、worker.rs:116 | 删纯分节标题×3（方向信息模块头已载）、init 帧行内复述×1 | 删×4 |
| proc+sandbox 组（6 文件/1652 注释行） | exec.rs:316/396/479、permissions.rs:273/335/382、apply_patch.rs:32/1036、proc_session.rs:1118/1656 | 删纯导航标题×10（均与紧随 doc 注释重复或零内容） | 删×10 |
| mcp+api 组（12 文件/623 注释行） | manager.rs:200/959、transport.rs:83/462/595/621、config.rs:32、call.rs:40、api.rs:602 | 删名称直译 doc×3、纯分节标题×4、复述×2 | 删×9（api.rs 为行尾注释，代码行内容不变——断言特判） |
| session+catalog+tui 组（5 文件/约 1139 注释行） | session.rs×8、catalog.rs×12、main.rs:74、tui_host.rs:34、view_image.rs:353/936 | 删纯分节标题×16、名称直译/复述×8 | 删×24 |

合计：**56 处**＝纯删 53＋压缩 1＋豁免属性 2。涉及 22 个源文件。

## 实际条数与清单「约 210」的差异说明

全库 6 组 47 文件、约 5,300 注释行逐行核实：绝大多数注释命中硬性保留规则——
规格编号引用（031/033/034/022/023/007/013/016/029/§x.x…）、审查编号＋约束
事实同句（审查 #N/P1-x/P2-x/N1）、fail-closed/安全语义（O_NOFOLLOW、
TOCTOU、密钥互斥、脱敏顺序）、平台差异（unix/windows 双版本）、预算口径、
测试场景说明。**清单「约 210」系审计期分类估计（约 90 复述＋约 50 分节标题＋
约 40 里程碑＋约 8 let_＋lint＋排序），无逐条明细落盘；本轮以「规格链接一律
保留」的更严口径实测，各目录 firm 候选率 0.2%–2%。** 存疑边界项（约 60 条，
含带 §/上游出处/测试组织价值的分节标题、pub API 名称直译 doc、唯一文档的
私有函数等）全部保守保留，逐条倾向已由探查记录给出，未凑数。

## 上报项（疑似代码问题，未动，交用户判断）

1. **tui_host.rs 约 :1947-1952/:2012（S9 后行号）**：测试
   `project_history_items_renders_checkpoint_boundary` **缺 `#[test]` 属性、从未
   运行**；其 032 场景文档与一个 `#[test]` 错位叠在相邻的
   `renders_orchestration_group` 上（后者带两块文档＋两个 `#[test]`）。属真实
   属性错位（S9 同类问题的测试版），修复牵动属性行——本轮注释清理范围外，
   上报待裁决。
2. proc_session.rs spawner_tx：Mutex 包裹 std mpsc Sender 的理由已过时
   （Sender 自 1.72 起 Sync），`Mutex` 本身或可移除——代码改动，登记未排期。
3. mcp/secrets.rs from_values 的 `sort_by_key`：所有消费方顺序无关，排序无效
   ——代码改动，登记未排期。

## 验证日志

- 非注释行零变化断言：**通过**（22 文件 diff 全部增删行均为注释/空行；豁免
  单列＝allow 属性行×2＋api.rs 行尾注释行对，两者代码内容零变化）。
- `cargo check --tests`：0 错误；警告 **605＝基线零新增**（删过时 allow 后
  无新增——message_label 非死代码、7 参不触发 lint，与预判一致）。
- lib 套件（单线程，沿 S8/S9 口径）：**309 passed / 0 failed / 1 ignored**。
- 代表集成套件：`--test e2e_acceptance` **150 passed / 0 failed**（真实 CLI＋
  PTY＋mock，覆盖 main/session/catalog/mcp/api 端到端）；`--test apply_patch`
  55/0（S9 轮已验，本批 apply_patch.rs 再动注释后 e2e 覆盖其真实路径）。

## 机器断言命令

```
git diff -U0 -- src/app/src > /tmp/s10.diff
# 断言：全部 +/- 行为注释或空行；豁免＝'#[allow(dead_code)]'、
# '#[allow(clippy::too_many_arguments)]' 删除、
# 'return Ok(None); // 超限即停' → 'return Ok(None);' → ASSERT OK
```
