# agentlab-demo 的开发容器
# 只做项目特有配置；通用工具都在 agentlab-base 基础镜像里。
FROM agentlab-base:latest

# Rust 工具链（rustup stable）：rustyline 18 需要 edition 2024（Rust 1.85+），
# Ubuntu 24.04 自带的 rustc 1.75 不满足，改用 rustup 跟随 stable。
# gcc + libc6-dev 必须显式列出：基础镜像没有任何 C 工具链，且
# --no-install-recommends 下 gcc 不会拉入 libc6-dev（原 apt rustc 的硬依赖），
# 缺它则链接时找不到 Scrt1.o/crti.o 和 -lutil 等系统库。
# curl/ca-certificates 基础镜像已带，rustup 直接可用。
# RUSTUP_HOME 固定在镜像内；运行时 compose 把 CARGO_HOME 指到
# /home/dev/.cargo 持久卷（registry 缓存持久化），与 rustup 本体不冲突。
USER root
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN apt-get update \
    && apt-get install -y --no-install-recommends gcc libc6-dev \
    && curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable --no-modify-path \
    && rm -rf /var/lib/apt/lists/*

# 033 工具沙箱：bubblewrap 持久安装（工具执行面隔离；TCHAT_SANDBOX=1 时按需调用）。
# 同 033 验收镜像（033-accept-bwrap.Dockerfile）的安装方式。
RUN apt-get update \
    && apt-get install -y --no-install-recommends bubblewrap \
    && rm -rf /var/lib/apt/lists/*
USER dev

# 其他语言/工具需要时在此追加（Node.js、build-essential 等）。
