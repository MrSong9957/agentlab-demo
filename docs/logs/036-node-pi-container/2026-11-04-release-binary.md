# Pi 独立发行包验证（2026-11-04）

- 范围：Dockerfile 中 Pi 官方 Release 安装、验收脚本路径断言与容器重建。
- 工作目录：宿主 `D:\AgentLab\projects\demo`；容器内 `/workspace`。
- `docker compose -f .devcontainer/compose.yaml config --quiet`：通过。
- 首次 `docker compose -f .devcontainer/compose.yaml build`：失败；压缩包保存为 `pi.tar.gz`，校验清单却引用 `pi-linux-x64.tar.gz`，故无法读取。修正文件名后再次构建：通过，`pi-linux-x64.tar.gz: OK`。此对照证明修复前构建无法通过校验。
- `docker run --rm --network none --env HOME=/tmp agentlab-demo-dev:latest sh -c 'set -e; test "$(id -un)" = dev; test "$(readlink -f "$(command -v pi)")" = /usr/local/lib/pi/pi; pi --version; pi --help >/dev/null; node --version; npm --version; cc-switch --version'`：通过；Pi 1.5.0、Node v24.22.2、npm 11.19.1、cc-switch 5.11.3。
- `docker exec agentlab-demo-dev bash -n /workspace/scripts/acceptance/036-node-pi-container.sh`：通过。
- `git diff --check -- .devcontainer/Dockerfile README.md docs/specs/036-node-pi-container.md scripts/acceptance/036-node-pi-container.sh`：通过。
- `docker compose -f .devcontainer/compose.yaml up -d`：重建并启动开发容器成功。`docker exec agentlab-demo-dev sh -c 'set -e; id -un; command -v pi; readlink -f "$(command -v pi)"; pi --version; pi --help >/dev/null; node --version; npm --version; cc-switch --version'`：通过；dev 用户执行 Pi 1.5.0，真实路径为 `/usr/local/lib/pi/pi`。
- 此前在旧容器运行 `unset TCHAT_SANDBOX && cargo check && cargo test --workspace --no-fail-fast`：退出码 1；最终报告 `js_orchestration_perf` 和 `proc_session` 两个测试目标失败。已确认前者 `c18_leaf_counts_1_4_31` 的 leaf1 p95 中位 448284µs 超过 250000µs 阈值；后者具体失败信息未保留，来源未确认。输出截断，无法可靠统计全量通过和忽略数量；至少 3 项忽略。应用户要求不再扩大测试范围。
- x86_64 已实际构建；arm64 未验证。完整 036 持久挂载及生命周期验收未执行。
