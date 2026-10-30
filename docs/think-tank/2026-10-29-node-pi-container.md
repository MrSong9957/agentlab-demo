# Node.js 与最新 Pi Agent 容器配置调研

日期：2026-10-29。目标：重建容器后自带最新稳定版 Pi 与兼容 Node，已有配置和会话仍可访问。

## 当前事实

- `.devcontainer/Dockerfile` 继承 `agentlab-base:latest`，安装 Rust、bubblewrap，最后切回 `dev`。
- `.devcontainer/compose.yaml` 已使用 Agent 命名卷、`/workspace` 绑定挂载、运行时 npm 缓存和代理；没有 Pi 卷，也没有 Node/Pi 安装步骤。
- 只读执行 `pi --version` 得到 `1.4.0`；命令路径为 `/home/dev/.local/bin/pi`，解析到 `/home/dev/.pi/agent/bin/pi`。仅凭这个路径不能证明安装来源。
- 当前 shell 和常见安装位置未发现 Node/npm。最新要求是安装最新 Pi 及相应 Node，因此不依赖手动安装版本作为镜像基准。
- [Pi 官网](https://pi.dev/)已使用 `@earendil-works/pi-coding-agent`，npm 安装要求 `--ignore-scripts`。[npm latest 元数据](https://registry.npmjs.org/@earendil-works/pi-coding-agent/latest)为 `1.4.0`，要求 Node `>=22.19.0`。

## 共识

软件进入镜像，数据进入命名卷，沿用现有非 root 用户。构建代理与运行代理分别处理；新卷权限和旧数据迁移必须有实际证据。

## 冲突与裁决

Node 官方归档和官方 Docker 镜像都可提供受支持运行时。选官方 Node 24 Debian 镜像阶段，复用其发布与架构处理，减少项目维护下载与校验脚本的工作；最终镜像继续使用现有基础镜像。

Pi 同时提供独立发行包和 npm 安装。采用官网 npm 路径，配合 Node 满足用户要求，减少独立发行包附带资源的管理。原始研究中的固定 `1.4.0` 只作调研样本，实施按构建时 `latest` 获取。

## 社区校准

本任务只固化已有开发容器的软件安装，按 `rules/计划前调研.md` 的简单任务约束压缩调研。未执行 last30days，不以社区意见作为安装或数据保留契约的依据；这部分未调研不能写成“社区无问题”。

## 组合方案

1. Node 24 LTS 镜像阶段提供 Node/npm，最终镜像安装官方 Pi 最新稳定 npm 包。
2. 系统安装路径优先，新增 `pi-home`，预建 dev 可写目录，部署前备份迁移旧数据。
3. 用隔离容器与卷验证版本、PATH、权限与重建后数据；补 README 的缓存刷新及更新说明。

## 背景知识

Docker 不会从旧容器可写层自动为新命名卷迁移数据。命名卷中的旧程序还可能遮盖新镜像中的安装，因此程序不能与配置混放作为交付方案。

`@latest` 也受 Docker 构建缓存影响。镜像只在重新执行安装步骤时解析最新版本，容器普通启动不会更新软件。

本地基础镜像 `agentlab-base:latest` 不应被一并要求远程拉取。主动更新先单独拉取官方 Node 镜像，再无缓存构建项目；[Docker 全局 `--pull`](https://docs.docker.com/reference/cli/docker/buildx/build/)会尝试拉取所有引用镜像。

项目规则引用的 `rules/规格文档.md` 不存在；本轮采用仓库实际提供的 `rules/规格文档实战法.md` 编写规格。本轮仅调研与规划，未安装、修改 Docker 配置、构建、迁移或执行测试。

## 原始输出索引

- [安装研究](raw/2026-10-29-node-pi-container-install.md)
- [构建与存储研究](raw/2026-10-29-node-pi-container-storage.md)
- [交付流程分析](raw/2026-10-29-node-pi-container-flow.md)
