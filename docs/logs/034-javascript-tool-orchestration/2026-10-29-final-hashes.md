# 034 最终候选内容哈希（2026-10-29 第三版，定向验收收口后）

含 C13（真实 016 终止语义 E2E：sentinel 存活证明＋terminate 后句柄保留＋write_stdin 续接 Running）与 C16（TUI 事件投影源码/参数/结果哨兵单测）补验后的终版候选。

## 核心文件
```
43ef653d0207256e15bb35dd073b6fc4cb2b75b15d753827ce59e6e27b4d51e5  src/app/src/js_orchestration/mod.rs
5e47fb294cfd8f72ecb1fd38e2d5351ad1f7b47674d33c16dc7aeef86e49af78  src/app/src/js_orchestration/args.rs
69ef8680d9bf10e7820da0680ef359ce535cb05cebb88fc6604792bb4cadf484  src/app/src/js_orchestration/protocol.rs
d7a413fd90040ac993aef070227b1444c578cdabca7e71d16a64bf0c4957b87a  src/app/src/js_orchestration/worker.rs
3c03597310e7e2cce475a2464d561cd474833e06380bb72c6ba5d61e9f70e4f5  src/app/src/js_orchestration/runtime.rs
20f564c6649f8028db85164a613cf6fd6e22bafc9148286499455c145da3be64  src/app/tests/js_orchestration.rs
9b0d112bfd9e0fa81f6d2ceccc8ff7f1ba29e6d2dbc97d36cf13ae8934316c8a  src/app/tests/js_orchestration_perf.rs
76538dfe5cd7abaab5ecc2cc0bc44964ea3654fdc099c4ea560c10a590e4cd02  src/app/tests/js_orchestration_security.rs
9741a82d5d9d80b8f60bc32b7377e27c97449dba93db472edf0a87336052f9c6  src/app/src/turn.rs
fcbd17dbb8189b6308a24624c4ce8e2475d158f91de41bd05a8f85c9741ce484  src/app/src/session.rs
a6661118bf14603d68d8c55bb19cec0b1fc4da34fe7de8d5f2b9f307d462326e  src/app/src/prompt.rs
530e6b027b8e13495dfff0b16a58636b3feac68f2f7fed18efde84d2696de3e9  src/app/src/tui_host.rs
0ae7182f6e7ef35c33fbd88ed81c61ccff97fcf9490d743b90b4c592b4be48f3  src/app/src/main.rs
d7faa2efcbd1dbd8043cd18903264768a2b8b16ecd1c1b2749c41cfd0921df32  src/app/src/catalog.rs
1e32eedd9d5a85df016d982d604871e5b6d5160cf5ac228ab8692977c29f5fc8  Cargo.lock
4f866c2ff61012550ca6f56c8c3a443594cd842879b059d20ecad9b29b9054d6  src/app/Cargo.toml
```
## 最终验证
env -u TCHAT_SANDBOX cargo check --locked && env -u TCHAT_SANDBOX cargo test --locked --workspace --no-fail-fast
结果（2026-10-29 收口跑）：3810 通过 / 1 失败 / 3 既有忽略——失败为 s140_request_permissions_cli_grant_and_enforce（031 权限 PTY e2e，并行负载抖动，单独复跑通过；同 s44 已知抖动模式，与 034 改动无交集）；其余全绿。

## 路径清单（互斥分类，2026-10-29 收口时点）
- 已跟踪修改 26：任务相关 22（代码 14＋测试 6＋规格/经验/清单 3＋依赖 2 减重叠）＋范围外用户编辑 4（AGENTS.md、sandbox-spec-sync、029、035——034 规格原为用户 M，本轮叠加实施更新）
- 新增未跟踪（任务）11 顶层路径/17 文件：js_orchestration/（5 模块）＋3 测试套件＋M0 原型归档（目录含 3 件）＋性能冻结与样本（2）＋审查闭环＋最终哈希＋安全报告
- 新增未跟踪（范围外，用户原有）8 项：.claude/、.compound-engineering/、think-tank、plan-preresearch、两版项目规则、计划前调研、大脑模型
- 前次报告「26 改＋8 新＋11 文档」的计数澄清：8 新（代码/测试）与 11 文档/证据互斥（分属 src/ 与 docs/ 未跟踪树）；11 为顶层路径数（展开 17 文件）。无重复计数。
- 暂存区：空（与用户原索引状态一致；未执行任何提交）
