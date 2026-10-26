# 033 真实沙箱验收镜像：dev 镜像 + bubblewrap（033-env-plan 变更 A）。
# 构建上下文 = 本文件所在目录（仅本文件，不含源码/凭据/个人配置）。
# docker build -f scripts/acceptance/033-accept-bwrap.Dockerfile \
#   -t agentlab-033-accept:latest scripts/acceptance/
FROM agentlab-demo-dev:latest

USER root
RUN apt-get update \
    && apt-get install -y --no-install-recommends bubblewrap \
    && rm -rf /var/lib/apt/lists/*
USER dev
