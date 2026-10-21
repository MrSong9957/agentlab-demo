# 历史验证摘要与原始日志索引

记录 2026-10-17 的两个独立验证轮。本次只整理既有结果，没有重新运行 Cargo 或改写历史测试结论。完整日志与原 README 固定在 Git 快照 `9e32a6347ae96d9ac2ff14d2655bdaf784d92fd9`；逐文件摘要见[留存清单](evidence/curation-manifest.json)。

<a id="v13"></a>

## v13

| 原执行命令 | 通过／失败／忽略 | 退出码 | 完整原件 |
|---|---|---:|---|
| `cargo check --workspace` | 不适用 | 0 | [cargo-check.log](../records/evidence-index.md#archive-a0cf2921f5274f11) |
| `cargo test --workspace --no-fail-fast` | 2775／0／3 | 0 | [cargo-test-workspace.log](../records/evidence-index.md#archive-3112d2460a791c49) |
| `cargo test --test e2e_acceptance` | 94／0／0 | 0 | [cargo-test-e2e.log](../records/evidence-index.md#archive-c696d2a63ab13661) |

原轮次说明（原 README 内容，保留中间失败及最终复跑说明）：

>
> - cargo-check.log：`cargo check --workspace`，退出码 0（文件尾 test_exit 行）。
> - cargo-test-workspace.log：`cargo test --workspace --no-fail-fast`，**2775 通过 / 0 失败 / 3 忽略**，退出码 0。计数核对：上轮（v12）总数 2774（2773 通过＋1 例 perf 负载偶发失败）＋本轮净增 1（s77 重写为 s78 不变＋新增 s79）＝2775 ✓。历史登记的 perf 偶发本轮未复现（未放宽门限、未设 ignored）。
> - cargo-test-e2e.log：`cargo test --test e2e_acceptance`，**94/94**（93－s77＋s78＋s79），退出码 0。
> - 过程中一次中间全量（2774/1）唯一失败＝footer_mode_snapshots 快照未更新（提示文案变更所致）——快照逐项核对后接受（footer_mode_history_search：`enter accept`；history_search ×3；unavailable＝Idle 态无提示、新旧一致），复跑全量通过。隔离复跑：footer_mode_snapshots 单独运行通过。
>
> ## 终版（复审 F4 断言补强后复跑，覆盖以上文件）
>
> - cargo-check.log：退出码 0。
> - cargo-test-workspace.log：**2775/0/3**，退出码 0（历史登记的 perf 偶发未复现）。
> - cargo-test-e2e.log：**94/94**，退出码 0。

[原 README](../records/evidence-index.md#archive-0797ef9a979a2045)。

<a id="v14"></a>

## v14

| 原执行命令 | 通过／失败／忽略 | 退出码 | 完整原件 |
|---|---|---:|---|
| `cargo check --workspace` | 不适用 | 0 | [cargo-check.log](../records/evidence-index.md#archive-588c7876011d9eb8) |
| `cargo test --workspace --no-fail-fast` | 2776／0／3 | 0 | [cargo-test-workspace.log](../records/evidence-index.md#archive-62f014dc4d16d64e) |
| `cargo test --test e2e_acceptance` | 95／0／0 | 0 | [cargo-test-e2e.log](../records/evidence-index.md#archive-cf13c1832efee9fa) |

原轮次说明（原 README 内容，保留中间失败及最终复跑说明）：

>
> - cargo-check.log：`cargo check --workspace`，退出码 0（文件尾 check_exit 行）。
> - cargo-test-workspace.log：`cargo test --workspace --no-fail-fast`，**2776 通过 / 0 失败 / 3 忽略**，退出码 0。计数核对：上轮（v13）2775＋本轮新增 s80＝2776 ✓。历史登记的 perf 负载偶发本轮未复现（未放宽门限、未设 ignored）。
> - cargo-test-e2e.log：`cargo test --test e2e_acceptance`，**95/95**（94＋s80），退出码 0。
>
> ## 终版（复审 P2 整改后复跑，覆盖以上文件）
>
> - cargo-check.log：退出码 0。
> - cargo-test-workspace.log：**2776/0/3**，退出码 0（perf 偶发未复现）。
> - cargo-test-e2e.log：**95/95**，退出码 0。
> - 过程记录：一次中间 e2e 全量出现 s26（resize 输入竞态压力用例）单发失败——隔离复跑 3/3 绿，负载偶发（与已登记 perf 偶发同类），复跑全量通过；如实登记，未改动该用例。

[原 README](../records/evidence-index.md#archive-0941c08a1592bd3e)。
