→ [返回汇总报告](../2026-10-29-cc-switch-container.md)

身份：官方安装契约核实。

目标：重建后可直接运行，避免镜像中的 root 安装与 dev 使用路径错位。

核实动作：①读取官方 README 的安装与存储说明。②读取 v5.11.3 标签的 install.sh。③核对 GitHub 最新稳定 Release 元数据及安装脚本、Linux x64/arm64 资产。

结果：命令为 cc-switch；默认安装到 HOME/.local/bin，支持 CC_SWITCH_INSTALL_DIR 与 CC_SWITCH_FORCE。Linux auto 选静态 musl。默认数据根为 HOME/.cc-switch，含 SQLite、设置、技能与备份。

前提：构建可访问官方 Release，提供 Bash、curl、tar、uname、mktemp；实际运行兼容性待新镜像验证。

验证：dev 版本/帮助命令、安装失败传播、隔离新卷初始化及重建后数据库可打开。本轮仅静态核实，未执行安装或运行测试。

来源：[README](https://github.com/SaladDay/cc-switch-cli/blob/main/README.md)、[安装脚本](https://github.com/SaladDay/cc-switch-cli/blob/v5.11.3/install.sh)、[发布元数据](https://api.github.com/repos/SaladDay/cc-switch-cli/releases/latest)。
