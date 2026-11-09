# 038 MCP 环境隔离探针：错误文本截断验证

日期：2026-11-05  
工作区：`/workspace`  
分支：`feat/038-github-actions-ci`  
基线：`57d5408`  
状态：未提交候选  
范围：`src/app/tests/sandbox_matrix_probes.rs::c_mcp_face_masks_env_and_keeps_explicit_env`

## 结果

原实现把 `cat` 的完整错误输出先交给 `head -c 90`，再将截断结果用于 `denied` 断言。用长度 150 字节的 `TMPDIR` 运行原实现时，测试在 `denied` 断言失败。失败输出中的 `ENV_READ` 只到临时路径的一部分，没有包含权限错误文本。输出没有假凭据标记；服务器 PID 为 1，HOME 为锚定根。该失败是展示截断造成的误判，没有显示环境隔离失效。

修复后，包装脚本先把 `cat` 的完整受控输出保存到 `env_read`，分别写出完整的 `ENV_READ` 和最多 90 字节的 `ENV_READ_DISPLAY`。Rust 断言继续要求完整 `ENV_READ` 含有 `denied`；失败消息只使用展示字段。

未缩短测试路径，未改 MCP 隔离实现。以下断言均保留：沙箱内假凭据标记不可读、权限拒绝文本、PID 为 1、HOME 为锚定根；普通模式能读到假凭据标记、PID 不为 1、HOME 为主进程值；显式 MCP 环境变量保留、污染变量不进入、TERM 传递、外部假凭据文件未改动，以及服务器进程回收检查。

## 定向验证

环境为 Linux，bubblewrap 可用。每次运行单独检查 `/sys/fs/cgroup/pids.current` 和 `/proc` 可见进程数，均低于 500；每次只运行一个精确用例，Cargo 构建作业数为 1，并有外层超时。

命令：

```sh
CARGO_TARGET_DIR=/workspace/target CARGO_BUILD_JOBS=1 CARGO_NET_OFFLINE=true \
TMPDIR="$(python3 -c 'print("/tmp/038-mcp-longpath-" + "x"*128)')" \
timeout -k 15 120 cargo test --locked --offline --test sandbox_matrix_probes \
c_mcp_face_masks_env_and_keeps_explicit_env -- --exact --nocapture
```

| 版本和路径 | 500 门禁起始值（tasks / 可见进程） | 结果 |
|---|---:|---|
| 原实现，长路径（`TMPDIR` 长 150 字节） | 236 / 3 | 退出 101；0 通过、1 失败、9 过滤。`denied` 断言失败，记录字段在权限错误出现前被截断。原始输出：`target/038-validation/mcp-longpath-old.log`。 |
| 修复后，长路径（相同 `TMPDIR`） | 310 / 3 | 退出 0；1 通过、0 失败、9 过滤。完整结果满足 `denied` 断言。原始输出：`target/038-validation/mcp-longpath-fixed.log`。 |
| 修复后，普通路径 | 267 / 3 | 退出 0；1 通过、0 失败、9 过滤。原始输出：`target/038-validation/mcp-normal-fixed.log`。 |

修复后的两次通过运行均覆盖沙箱和普通模式。普通模式要求记录中出现唯一假凭据标记，作为正文确实可读的正向对照；沙箱模式仍用完整记录拒绝该标记，并检查隔离身份和环境。原实现的长路径运行在沙箱分支先失败，没有执行到普通分支。用例仍能发现正文读取成功或隔离配置失效。

本轮只运行该精确用例的原实现复现、长路径修复验证和普通路径修复验证。没有运行 L3、全量测试或其他失败项；没有修改 worker 回收实现；没有提交、合并、推送或触发 CI。
