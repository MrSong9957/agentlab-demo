# 移除 Python 部分，项目转为纯 Rust（2026-10-07）

## 设计依据

- 用户决定：项目只使用纯 Rust，Python 验收示例（`app.py` 的 `add` 函数及其 pytest 测试）不再需要。
- 删除清单：`app.py`、`tests/test_app.py`（pytest 用例 2 条）、`pyproject.toml`、`uv.lock`、`.venv/`、`__pycache__/`。
- 连带更新引用：`README.md`"容器内测试"一节改为 `cargo test`；`AGENTS.md` 第 7 节技术栈/模块边界/命令表删除全部 Python 条目（全量验证口径改为仅 `cargo check && cargo test`）；`.devcontainer/compose.yaml` 删除纯 uv/pip 工具链环境变量 `UV_CACHE_DIR`、`PIP_CACHE_DIR`、`UV_LINK_MODE`（npm/pnpm/playwright 缓存变量与浏览器 E2E 相关，保留）；`.gitignore` 删除 `__pycache__/`、`*.pyc`、`.venv/` 三行。
- 有意保留：`logs/security-20261005/attack.py` 是 003 安全修复的历史验证证据，不属项目 Python 部分；`docs/`、`logs/` 中提及 pytest 的历史文档不回改。

## 验证结果

- `cargo check --locked` 通过；`cargo test --locked` 单元 32 + 集成 38 = 70 全绿、0 警告（与删除前 Rust 侧完全一致，无 Rust 用例变动）。
- 源码与配置层（src/、tests/、Cargo.toml、README、AGENTS.md、.gitignore、.devcontainer/）grep 无 Python 工具链残留；AGENTS.md:90 刻意保留一行移除说明。
- Dockerfile 与 compose 原本未安装 Python 构建步骤（grep 零命中），容器侧无需其他改动；已设置的环境变量在容器重建后生效。

> 2026-10-19 归档位置更新：上文保留的历史攻防脚本现由 Git 快照 `9e32a63` 保存，见[安全原件索引](evidence-index.md#security-originals)；它仍是历史证据，不属于当前项目的 Python 运行或测试入口。
