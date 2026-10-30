→ [返回汇总报告](../2026-10-29-node-pi-container.md)

# Pi 1.4.0 独立版固化调研

- 我是谁：核对官方产物的运行时研究员。
- 离谱目标：保留当前版本，重建即用，程序升级不被数据卷遮住。
- 我的方案：①固定官方 `v1.4.0` 对应架构压缩包；②校验 SHA-256；③整包解压到 `/usr/local/lib/pi`，链接 `/usr/local/bin/pi`；④仅持久化 `/home/dev/.pi` 数据，检查旧启动器不抢占命令。
- 成立前提：当前独立版身份待证实。官方独立版内置 Bun；npm 版另需 Node ≥22.19。整包含主题、WASM、原生组件，不能只复制单个文件。
- 验证方法：隔离数据目录，检查版本、离线帮助、资源加载和重建保留；本轮未下载产物或执行验证。

## 官方来源与版本固定依据

- [v1.4.0 Release](https://github.com/earendil-works/pi/releases/tag/v1.4.0) 与 [发布元数据](https://api.github.com/repos/earendil-works/pi/releases/tags/v1.4.0)：正式版本，提供 Linux x64、arm64 包以及 `SHA256SUMS`。
- x64 固定资产：`https://github.com/earendil-works/pi/releases/download/v1.4.0/pi-linux-x64.tar.gz`；API 报告 SHA-256：`f2e4e4329603223d03260cdb8bd5033dbdc1f73ba58ab8939db2e56546e115d2`。
- arm64 固定资产：`https://github.com/earendil-works/pi/releases/download/v1.4.0/pi-linux-arm64.tar.gz`；API 报告 SHA-256：`fa5829c7ef83ffeaebba015715c8306220f89b2aa8a00154efbba98dd5440156`。
- [官方打包脚本](https://github.com/earendil-works/pi/blob/v1.4.0/scripts/build-binaries.sh)：使用 Bun `--compile`；tar 内有一层 `pi/` 包装目录，含程序、`package.json`、主题、图片、WASM、原生平台组件、文档与示例。整个包装目录的内容应保留在同一安装根目录。
- [官方发布工作流](https://github.com/earendil-works/pi/blob/v1.4.0/.github/workflows/build-binaries.yml)：对发布资产执行 `sha256sum` 生成 `SHA256SUMS`。以上哈希本轮仅从 HTTPS API 读取；未下载包做实测校验，也未核验独立签名。
- [官方路径源码](https://github.com/earendil-works/pi/blob/v1.4.0/packages/coding-agent/src/config.ts)：Bun 独立版按 `dirname(process.execPath)` 查找旁边资源，支持 `PI_PACKAGE_DIR` 显式覆盖；因此整包移到系统目录在源码层面可行，软链接调用的资源解析仍需实现阶段验证。
- [官方 Quickstart](https://github.com/earendil-works/pi/blob/v1.4.0/packages/coding-agent/docs/quickstart.md)：npm 包为 `@earendil-works/pi-coding-agent`，使用 `--ignore-scripts`，但普通 npm 安装不锁传递依赖；Node 是 npm 方案的要求，不是独立二进制本身的要求。
- [官方配置说明](https://github.com/earendil-works/pi/blob/v1.4.0/packages/coding-agent/docs/configuration.md) 与 [会话说明](https://github.com/earendil-works/pi/blob/v1.4.0/packages/coding-agent/docs/sessions.md)：默认用户目录 `~/.pi/agent`，包含认证、配置与默认会话；配置/会话目录可能被环境变量或设置覆盖。

系统安装根目录不放在持久卷内；新增镜像的程序应由构建替换，数据卷继续挂载原数据。当前容器的单文件来源不能只凭路径与版本判定；若旧数据卷中保留 `agent/bin/pi`，不能让旧 `~/.local/bin/pi` 启动器优先命中它。
