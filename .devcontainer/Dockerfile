# agentlab-demo 的开发容器
# 只做项目特有配置；通用工具都在 agentlab-base 基础镜像里。
FROM node:24-bookworm-slim AS node
FROM agentlab-base:latest

# Rust 工具链（rustup stable）：rustyline 18 需要 edition 2024（Rust 1.85+），
# Ubuntu 24.04 自带的 rustc 1.75 不满足，改用 rustup 跟随 stable。
# gcc + libc6-dev 必须显式列出：基础镜像没有任何 C 工具链，且
# --no-install-recommends 下 gcc 不会拉入 libc6-dev（原 apt rustc 的硬依赖），
# 缺它则链接时找不到 Scrt1.o/crti.o 和 -lutil 等系统库。
# libstdc++6/libatomic1 为下方官方 Node 二进制提供运行依赖。
# curl/ca-certificates 基础镜像已带，rustup 直接可用。
# RUSTUP_HOME 固定在镜像内；运行时 compose 把 CARGO_HOME 指到
# /home/dev/.cargo 持久卷（registry 缓存持久化），与 rustup 本体不冲突。
USER root
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:/usr/local/bin:$PATH
RUN apt-get update \
    && apt-get install -y --no-install-recommends gcc libc6-dev libstdc++6 libatomic1 \
    && curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable --no-modify-path \
    && rm -rf /var/lib/apt/lists/*

# 033 工具沙箱：bubblewrap 持久安装（工具执行面隔离；TCHAT_SANDBOX=1 时按需调用）。
# 同 033 验收镜像（033-accept-bwrap.Dockerfile）的安装方式。
RUN apt-get update \
    && apt-get install -y --no-install-recommends bubblewrap \
    && rm -rf /var/lib/apt/lists/*

# Node/npm 来自官方 glibc 镜像，只复制运行所需文件，保留基础镜像其他工具。
# Node/npm 供容器内通用开发使用；pi 已改用官方 Release 独立二进制，不再经 npm 安装
# （036 规格修订，2026-11-04）。
COPY --from=node /usr/local/bin/node /usr/local/bin/node
COPY --from=node /usr/local/lib/node_modules/npm /usr/local/lib/node_modules/npm
RUN ln -sf ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
    && ln -sf ../lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx

# pi 官方 Release 独立二进制（Bun 编译自包含，不依赖 Node/npm；按用户要求使用）。
# tar 内含一层 pi/ 包装目录（程序＋theme/assets/
# native 等资源须同目录，官方按 dirname(execPath) 查找资源），整包解到
# /usr/local/lib/pi，/usr/local/bin/pi 符号链接指向真实路径（符号链接调用加载
# 资源已实测正常）。SHA256SUMS 由官方发布工作流生成，构建期强制校验。
RUN set -eux; \
    arch="$(uname -m)"; \
    case "$arch" in \
        x86_64) pi_arch="x64" ;; \
        aarch64) pi_arch="arm64" ;; \
        *) echo "unsupported arch: $arch" >&2; exit 1 ;; \
    esac; \
    curl -fsSL "https://github.com/earendil-works/pi/releases/latest/download/pi-linux-${pi_arch}.tar.gz" -o "/tmp/pi-linux-${pi_arch}.tar.gz"; \
    curl -fsSL "https://github.com/earendil-works/pi/releases/latest/download/SHA256SUMS" -o /tmp/pi-SHA256SUMS; \
    (cd /tmp && grep "pi-linux-${pi_arch}.tar.gz" pi-SHA256SUMS | sha256sum -c -); \
    tar -xzf "/tmp/pi-linux-${pi_arch}.tar.gz" -C /tmp; \
    cp -a /tmp/pi/. /usr/local/lib/pi/; \
    ln -s /usr/local/lib/pi/pi /usr/local/bin/pi; \
    rm -rf /tmp/pi "/tmp/pi-linux-${pi_arch}.tar.gz" /tmp/pi-SHA256SUMS

# 分开下载和执行，下载失败时构建立刻失败；参数仅作用于官方安装脚本。
RUN curl -fsSL https://github.com/SaladDay/cc-switch-cli/releases/latest/download/install.sh \
        -o /tmp/cc-switch-install.sh \
    && CC_SWITCH_INSTALL_DIR=/usr/local/bin CC_SWITCH_FORCE=1 bash /tmp/cc-switch-install.sh \
    && rm /tmp/cc-switch-install.sh

# 程序留在镜像，用户数据由 compose 的持久目录挂载提供。
RUN install -d -m 0700 -o dev -g dev /home/dev/.pi /home/dev/.cc-switch
USER dev
