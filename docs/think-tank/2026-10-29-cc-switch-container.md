# CC-Switch CLI 容器安装调研

日期：2026-10-29。目标：将用户指定的 CC-Switch 官方安装方式加入现有开发容器计划，软件随镜像重建，数据保留。

## 官方事实与采用方案

- [官方 README](https://github.com/SaladDay/cc-switch-cli/blob/main/README.md)给出的安装命令与用户提供的命令一致，执行命令名为 `cc-switch`。
- [v5.11.3 安装脚本](https://github.com/SaladDay/cc-switch-cli/blob/v5.11.3/install.sh)默认安装到 `$HOME/.local/bin`，支持 `CC_SWITCH_INSTALL_DIR` 和 `CC_SWITCH_FORCE=1`。构建时使用系统安装目录，且只为安装进程设置这些参数。
- 同一脚本在 Linux 自动选择静态 musl 的 x64 或 arm64 资产，不回退 glibc；无需额外安装 Node/Rust 编译工具来构建 CC-Switch。Node 的 glibc 安装要求与这个独立 Rust 程序分别处理。
- [默认数据位置](https://github.com/SaladDay/cc-switch-cli/blob/main/README.md#configuration-files)为 `~/.cc-switch`，包括 SQLite、设置、备份和技能源。新增 `cc-switch-home` 命名卷，沿用默认路径，不设置新的配置路径覆盖变量。
- [当前稳定 Release](https://github.com/SaladDay/cc-switch-cli/releases/tag/v5.11.3)为 v5.11.3，只是核实样本；未来构建使用 `latest`，版本证据来自实际安装。

## 共识与取舍

复用官方脚本，避免另写下载器；程序进系统目录、数据进用户卷。现有基础镜像、Node/Pi 方案和其他 Agent 卷保持原计划。CC-Switch 仅安装，不运行供应商切换、真实配置同步或代理功能。

安装下载和执行分别检查失败，以保证镜像构建结果可信。主动更新继续用现有计划的缓存刷新方式，不在容器启动时升级。

## 调研范围与限制

这是现有计划的一项安装扩充，按简单任务约束直接核实官方材料，不新增多领域专家或 last30days 调研。Release 安装脚本直链读取失败，只读核实官方标签源码与发布元数据，未下载或运行程序，不能据此报告已安装或已通过。

验收覆盖系统路径、dev 权限、非交互安装失败传播、版本及数据库卷生命周期。所有数据使用临时目录，禁止借验收触碰真实 Agent 配置。

## 原始输出索引

- [官方资料核实记录](raw/2026-10-29-cc-switch-container-official.md)
