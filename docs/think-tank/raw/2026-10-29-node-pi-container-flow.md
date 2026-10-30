→ [返回汇总报告](../2026-10-29-node-pi-container.md)

我是谁：容器交付流程审查员。

离谱目标：重建后即用，同时保住已有会话。

我的方案：①镜像预建 dev 可写的 `.pi`，验证新卷及旧卷权限。②旧容器若已有 `.pi`，先备份再迁卷，重建不删卷。③构建代理单独传入；验收隔离镜像、容器名、卷和项目目录，在 Windows 宿主处理现有 seccomp 路径。

成立前提：确认 Pi 身份；宿主 Docker 及基础镜像可用。

验证方法：dev 运行 node/npm/pi 版本命令；写假配置与会话标记，停启及删容器重建后比对；核对既有容器与卷未变。本轮均未执行。

依据：`.devcontainer/Dockerfile` 的 `USER dev`；`.devcontainer/compose.yaml` 的固定 `container_name`、命名卷、运行时代理和 Windows seccomp 路径。
