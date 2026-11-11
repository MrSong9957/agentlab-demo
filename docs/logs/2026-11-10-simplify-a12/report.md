# A12 恢复从未运行的测试处理记录（2026-11-11）

## 现场（修复前）

`tui_host.rs`（S10 提交 `c5072d1` 后 :1946-1952）：032 检查点场景文档＋一个
`#[test]`＋034 编排场景文档＋一个 `#[test]` 四行连排，全部叠在
`project_history_items_renders_orchestration_group` 头上；
`project_history_items_renders_checkpoint_boundary`（:2011 附近）**无 `#[test]`**
——从未被收集、从未运行，且以私有未调用 fn 形态触发 dead_code「never used」
警告（605 基线的构成之一）。

## 修复

1. :1946-1948 的 032 场景文档＋孤儿 `#[test]` 随迁至
   `renders_checkpoint_boundary` 上方（文档随迁，文字零改动）。
2. `renders_orchestration_group` 保留 034 文档＋单个 `#[test]`。
3. 未改任何测试断言与产品代码。

## 核实结论

- **实跑通过**：`cargo test --lib project_history_items` → 8 passed / 0 failed
  （含 `renders_checkpoint_boundary ... ok`，首次运行即绿）；单测过滤重跑
  3/3。断言与当前行为一致——无过时断言、无真实缺陷（未触发阻断）。
- **收集清单对照**（`--list` 集合差，stash 隔离测量）：
  - 修复前：309 个唯一测试名；`renders_checkpoint_boundary` 不在列；
    **`renders_orchestration_group` 因双重 `#[test]` 被 libtest 重复收集为
    两行同名条目**（表面总数 310 的来源；早前 comm 对比出现的「假差异」即
    同名重复行＋排序对齐错乱的产物，已用 python 集合差裁决）。
  - 修复后：310 个唯一测试名，唯一新增＝`renders_checkpoint_boundary`，
    零丢失，同名重复条目消除。

## 验证

- `cargo check --tests`：0 错误。
- 警告：**604＝基线 605−1**，对应关系已坐实——消失的即修复前
  `renders_checkpoint_boundary` 的 dead_code「never used」（stash 对照：
  修复前 605 且该警告命中 1 次；修复后 604 且命中 0 次）。零新增。
- lib 套件（单线程，沿 S8 口径）：**309 passed / 0 failed / 1 ignored**
  （310 唯一名全绿，含恢复的测试）。
- tui_host 测试组：`cargo test --lib tui_host::tests` → **14 passed / 0 failed**。
