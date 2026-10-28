#!/usr/bin/env bash
# 033 工具沙箱 dev 容器部署脚本——在 Windows 宿主 Git Bash 中执行（容器内无 docker CLI）。
# 前置：容器内已完成配置修改与 seccomp 生成（见 docs/logs/033-tool-sandbox/2026-10-27-dev-deploy.md）。
# 行为：版本守卫 → 镜像回滚点 → 构建新镜像 → 重建 dev 服务（保留全部卷，不用 down -v，不动其他服务）。
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# docker 调用点禁 MSYS 路径转换（033 验收 S3：全局禁用会破坏其他命令，按调用点处理）。
# 传给 docker.exe 的文件路径须为 D:/ 盘符正斜杠形式（/d/... 在 MSYS_NO_PATHCONV=1
# 下不转换、docker.exe 解析不了）；cygpath -m 完成该转换（宿主验收实测配方）。
# compose 文件必须显式 -f（compose.yaml 在 .devcontainer/ 下，默认搜索当前目录找不到）；
# 项目名由 compose.yaml 顶层 name: agentlab-demo 固定，与既有部署一致。
ROOT_WIN=$(cygpath -m "$ROOT")
DC() { MSYS_NO_PATHCONV=1 docker compose -f "$ROOT_WIN/.devcontainer/compose.yaml" "$@"; }

echo "== 0. 引擎版本守卫 =="
ENGINE_VER=$(MSYS_NO_PATHCONV=1 docker version --format '{{.Server.Version}}')
echo "Engine: $ENGINE_VER"
if [ "$ENGINE_VER" != "29.8.2" ]; then
  echo "FAIL: seccomp 基线按 docker-v29.8.2 → moby/profiles/seccomp v0.2.3 锁定生成，" >&2
  echo "当前引擎 $ENGINE_VER ≠ 29.8.2——不得沿用未知版本的策略基线（fail closed）。" >&2
  echo "请按 docs/logs/033-tool-sandbox/2026-10-27-dev-deploy.md「基线再生成」重做后再部署。" >&2
  exit 1
fi

echo "== 1. seccomp 文件就绪检查（由执行 docker 的 CLI 在宿主侧读取） =="
SEC="$ROOT/.devcontainer/033-seccomp.json"
[ -r "$SEC" ] || { echo "FAIL: $SEC 不可读" >&2; exit 1; }
echo "OK: $SEC ($(wc -c < "$SEC") bytes)"

echo "== 2. 固化回滚材料（本轮修改前快照＋正向补丁） =="
# 前提实证（2026-10-27）：Dockerfile/compose.yaml 相对 HEAD 的 diff 仅含 033
# 改动（bubblewrap 层/security_opt/TCHAT_SANDBOX），故 HEAD 版本＝本轮修改前快照；
# 回滚不用整文件 git checkout——见部署记录「回滚」节（反向补丁优先，可检测冲突）。
RB="$ROOT/.devcontainer/033-rollback"
mkdir -p "$RB"
git show HEAD:.devcontainer/compose.yaml > "$RB/compose.yaml.pre-033"
git show HEAD:.devcontainer/Dockerfile > "$RB/Dockerfile.pre-033"
git diff -- .devcontainer/Dockerfile .devcontainer/compose.yaml > "$RB/033-host-changes.patch"
[ -s "$RB/033-host-changes.patch" ] || { echo "FAIL: 正向补丁为空（033 配置改动不在工作区？）" >&2; exit 1; }
echo "回滚材料: $RB (compose.yaml.pre-033 / Dockerfile.pre-033 / 033-host-changes.patch)"

echo "== 3. 当前运行中容器（仅展示，脚本不处理任何其他容器） =="
MSYS_NO_PATHCONV=1 docker ps --format '{{.Names}} | {{.Image}} | {{.Status}}'

echo "== 4. 镜像回滚点（已存在则保留旧 tag，防重复执行污染） =="
if MSYS_NO_PATHCONV=1 docker image inspect agentlab-demo-dev:pre-033-rollback >/dev/null 2>&1; then
  echo "回滚点已存在，保留: agentlab-demo-dev:pre-033-rollback（不覆盖）"
else
  MSYS_NO_PATHCONV=1 docker tag agentlab-demo-dev:latest agentlab-demo-dev:pre-033-rollback
  echo "回滚镜像已备: agentlab-demo-dev:pre-033-rollback"
fi

echo "== 5. 构建新镜像（dev + bubblewrap） =="
DC build dev

echo "== 6. 重建 dev 服务（保留全部卷；仅 dev，不用 down -v） =="
DC up -d dev

echo "== 7. 部署确认 =="
DC ps dev
MSYS_NO_PATHCONV=1 docker inspect agentlab-demo-dev --format 'SecurityOpt={{.HostConfig.SecurityOpt}}'
echo
echo "部署完成。下一步（容器内）：bash /workspace/.devcontainer/verify-033-deploy.sh"
echo "回滚：见 docs/logs/033-tool-sandbox/2026-10-27-dev-deploy.md「回滚」节。"
