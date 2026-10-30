→ [返回汇总报告](../2026-10-29-node-pi-container.md)

# Docker 安装与持久化调研

我是谁：容器构建与存储研究员。

离谱目标：从手动重装变为重建即用，依靠镜像和命名卷分工。

我的方案：①保留基础镜像，选官方 Node 24 发布包并核验 SHASUMS；亦可从 Debian 版官方 Node 镜像精确复制 Node/npm，避免 Alpine/musl 和整目录覆盖。②镜像内全局安装锁定版 pi，保证非登录 shell 可执行。③预建 dev 所有的 `.pi`，挂 `pi-home`；旧数据先备份迁移。④构建代理单独传入，缓存与运行代理沿用 Compose。

成立前提：目标架构及动态库匹配；保留同一卷，不执行删卷操作。

验证方法：dev 直接运行三条版本命令，空卷写入后重建容器核对；本轮未执行。

## 官方依据

- [Node 官方下载与签名校验入口](https://nodejs.org/en/download/archive/v24.22.1)：列有 Linux x64、arm64 归档及 Signed SHASUMS。该页同时列出更新补丁版本；实施时重新核实并锁定，本文不以此归档版本作为必须采用版本。
- [官方 Node Debian 镜像 Dockerfile](https://github.com/nodejs/docker-node/blob/main/24/bookworm-slim/Dockerfile)：可复用其官方发布包、架构选择和校验方式。跨镜像复制仍须验证最终 Ubuntu 基础镜像的动态库。
- [Docker 多阶段构建](https://docs.docker.com/build/building/multi-stage/)：外部镜像可作为复制来源，最终阶段继续使用现有基础镜像。
- [Docker 卷生命周期与初始化](https://docs.docker.com/engine/storage/volumes/#mounting-a-volume-over-existing-data)：新空卷从镜像中的挂载目录填充；卷独立于容器存在。旧容器可写层不属于新镜像或新卷，迁移步骤不能省略。目录权限是否正确必须由新卷、dev 写入实测，文档不替代该证据。
- [Docker 构建代理参数](https://docs.docker.com/build/building/variables/#proxy-arguments)：使用预定义代理 build args；默认避免进入 history/cache，不在 Dockerfile 中引用或持久化为 ENV。镜像拉取由 Docker daemon 的代理负责，亦不同于容器运行代理。
- [npm 全局安装与缓存目录](https://docs.npmjs.com/cli/v11/configuring-npm/folders/)：Unix 全局程序在 prefix/bin，包在 prefix/lib/node_modules，缓存另由 cache 设置。现有 Compose 的 `npm_config_cache=/cache/npm` 不代表构建时已经有该卷或配置。

读取范围：`.devcontainer/Dockerfile`、`.devcontainer/compose.yaml`、`README.md`。未读取真实 `.pi` 配置或凭证，未安装、构建、运行测试。
