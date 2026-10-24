# 022 MCP 性能基准工具（归档）

- 用途：`docs/records/2026-10-24-022-mcp-performance.md` 的可复跑测法；原始基线数据在 `docs/testing/perf/022/baseline-{overhead,mcp}/`。
- 环境：同机 release 构建、`--locked --offline`、专用 `--target-dir`（禁止共享 `/workspace/target`）。

## 组件

| 文件 | 作用 |
|---|---|
| `bench_overhead.py` | 接入开销 A/B（两版本 CLI 二进制；startup／chat_turn；预热2＋N=30 交替） |
| `run_mcp_bench.py` | MCP 场景梯度运行器（发现／稳态快照／重列／往返／取消／关闭 × tools 8/64/512 × schema 256B/4KiB） |
| `mcp_server.py` | 本地受控 stdio MCP 夹具（--tools/--schema-bytes/--scenario/--delay-ms） |
| `mock_model.py` | 本地 mock 模型服务（固定终文；端口参数） |
| `mcp_bench.rs` | 管理器 API 计时驱动（example 源；非构建目标；含 `probe_notify_turn` 探针场景） |
| `notify_probe_run.py` | 通知生效轮次对照探针运行器（基线 vs 候选 × NOTIFY_DELAY_MS 扫描；delay 须经 mcp.json `env_vars` 显式转交，夹具缺失即报错） |

脚本默认引用 `/tmp/perf-target-old|new`（隔离构建产物）与 `/tmp/perf-harness`（工作目录）；按下方流程重建。

## 重建与复跑

```bash
# 1) 隔离 worktree（历史版本与候选；不触碰当前工作区）
git worktree add --detach /tmp/perf-f662cf6 f662cf6e4a7881923487ac40b395a20efdb77c31
git worktree add --detach /tmp/perf-4fbf052 4fbf052bdabbb54a3e637906ce32ac48c56b8e72   # 或候选提交
# 2) release 构建（专用 target-dir；两版本同参数）
(cd /tmp/perf-f662cf6 && CARGO_TARGET_DIR=/tmp/perf-target-old cargo build --release --locked --offline)
(cd /tmp/perf-4fbf052 && CARGO_TARGET_DIR=/tmp/perf-target-new cargo build --release --locked --offline)
# 3) MCP 驱动 example（复制进隔离 worktree 后构建）
mkdir -p /tmp/perf-4fbf052/src/app/examples /tmp/perf-4fbf052/tests-mcp-fx
cp scripts/perf/mcp_bench.rs /tmp/perf-4fbf052/src/app/examples/
cp scripts/perf/mcp_server.py /tmp/perf-4fbf052/tests-mcp-fx/   # example 以相对路径引用
(cd /tmp/perf-4fbf052 && CARGO_TARGET_DIR=/tmp/perf-target-new cargo build --release --locked --offline --example mcp_bench -p terminal-chat)
# 4) 运行（结果默认写 /tmp/perf-harness/results-*；与归档基线同口径对比）
mkdir -p /tmp/perf-harness && cp scripts/perf/*.py /tmp/perf-harness/
python3 /tmp/perf-harness/bench_overhead.py
python3 /tmp/perf-harness/run_mcp_bench.py
# 5) 结束后清理 worktree
git worktree remove --force /tmp/perf-f662cf6; git worktree remove --force /tmp/perf-4fbf052
```

## 基线哈希（2026-10-24 采集）

| 产物 | SHA-256 |
|---|---|
| terminal-chat f662cf6 (release) | `473613efb1934fd28f66ff2e0038333a74a69de275be9543ecfd3a52ca14b380` |
| terminal-chat 4fbf052 (release) | `398e670c54a69356f525290a227c335e5808192bf477ac0cdb26f5506899b261` |
| mcp_bench example (4fbf052) | `7b3190d9efc1216b950fde9cd55133743f332b87249c95e1b53847c29a94ea2b` |

原始数据文件哈希清单见 `docs/testing/perf/022/SHA256SUMS.txt`。
