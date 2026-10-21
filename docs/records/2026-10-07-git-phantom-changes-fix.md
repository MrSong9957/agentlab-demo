# Windows Git 与容器 Git 幻影变更修复（2026-10-07）

## 现象与诊断

- 用户 VS Code（宿主机 Windows git，打开 `D:\AgentLab\projects\demo`）显示 8 个虚假 M；容器内 `git status` 同一仓库为干净（0 项）。两侧共用同一 `.git` 目录。
- 根因（三因素叠加）：index 的 stat 元数据由 Linux 侧写入，Windows git 全部失配被迫重算；Windows git 全局 `core.autocrlf=true`（`git diff` 的 "LF will be replaced by CRLF" warning 实证）而仓库无 `.gitattributes`，3 个 CRLF 入库 md（Agent开发核心宗旨、001、002 规格）被判为已修改；`core.filemode=true` 下 9p/drvfs 两侧权限位不可靠，产生 5 个 mode 幻影（含从未改动的 `main.rs`、`Dockerfile`）。
- 8 = 3 行尾幻影 + 5 mode 幻影。容器内 `git diff --cached --ignore-cr-at-eol` 证明暂存变更纯为行尾。

## 修复（提交 46d3fb2）

- `git config core.filemode false`（仓库级配置，不入库）：消除 mode 幻影。
- 新增 `.gitattributes`（`* text=auto`）入库：行尾规则由仓库统一裁决，两侧 git 判定一致。
- `git add --renormalize .` + 提交：3 个 CRLF 文件入库内容规范化为 LF；工作区文件未被重写。

## 验证结果

- 规范化前后内容逐字一致（`--ignore-cr-at-eol` 后 diff 仅剩 `.gitattributes`）。
- 提交后 `git status` 干净，CRLF 入库文件数 0。
- `/vault`（Obsidian 仓库）17 个真实未提交修改全程未动，提交前后计数一致。
- **用户确认：VS Code 刷新后 8 个虚假 M 全部消失。**

## 后续注意

- 在 VS Code 中不要对该仓库的幻影文件点"放弃更改"（修复前），Windows git 会按其行尾规则重写文件造成真实 churn；现在规则已统一，风险解除。
- `/vault` 仓库的 17 个真实修改是独立事项，待用户自行处理。
