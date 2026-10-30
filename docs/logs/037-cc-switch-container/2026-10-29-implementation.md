# CC-Switch 配置实施证据

- 日期：2026-10-29；工作目录：`/workspace`。
- 官方 latest 安装脚本已写入 Dockerfile；只为脚本执行进程设置 `CC_SWITCH_INSTALL_DIR=/usr/local/bin` 和 `CC_SWITCH_FORCE=1`。
- 脚本先下载成功才执行，下载或执行失败都会阻断构建；未在本次运行该安装脚本。
- Compose 配置 `.container-state/cc-switch` 到 `/home/dev/.cc-switch` 的宿主绑定挂载。旧数据在实施中出现（含活跃 SQLite 数据库），已在写入进程停止后完成最终迁移与符号链接切换，旧目录改名保留备份；过程与校验证据见 [036 公共实施日志](../036-node-pi-container/2026-10-29-implementation.md)，不重复抄写。
- 配置检查、数据目录切换、安全测试、审查及全量回归统一引用 [036 公共实施日志](../036-node-pi-container/2026-10-29-implementation.md)，不重复抄写。
- 用户明确不构建镜像或重建容器，真实安装版本与新容器数据库运行验收未执行。
