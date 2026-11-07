#!/usr/bin/env bash
# 环境恢复：保全 → 核验 → 原地重启一次。（R3 修订；问题清单见 STATUS.md R2/R3 节）
# 运行位置：宿主侧（WSL2 或 Windows Git Bash 均可；已实测 MINGW64＋docker.exe 直连）。
#   强制前置门：同目录 validate-result.txt 含 VALIDATION PASS
#   （由 validate_recovery.sh 模拟验证生成），否则拒绝启动。
# 失败语义：任一保全条件未证实 → 退出且不重启；重启后仅当容器 ID 未变且 Status=running 才登记重启完成。
# 授权：保全全部通过后原地重启一次（既有授权，仅 docker restart，不删除/不重建）。
# 一致性（R2）：git status 只作变更登记，不作为保全条件；保全条件＝两工作目录＋取证文件的
#   内容双读逐字节核验。写入控制主证据＝docker pause 冻结（inspect 确认），可选人工预暂停；
#   重启后的 verify.sh 仅作回归检测，不补充重启前条件。
# 敏感排除（R2）：workspace 主备份流上排除；/tmp 侧各通道在提取侧排除；已跟踪的非敏感测试
#   夹具不回填副本——其当前内容与删除状态独立保全（fixtures-current.tgz / fixtures-status.txt）。
# 仅限模拟验证使用的覆盖项（真实执行不得设置）：RECOVERY_SRC、RECOVERY_BASELINE、
#   RECOVERY_MIN_WS_FILES、RECOVERY_MIN_WT_FILES、RECOVERY_BACKUP_ROOT。
set -euo pipefail
# 仅 docker.exe 需要 MSYS 路径改写屏蔽（容器内路径 CID:/tmp/… 不能被转成宿主路径）；
# git.exe 反而依赖 MSYS 路径解析（-C /tmp/…）——绝不全局 export（R3 修复）。
# dk 实现为真实脚本而非 bash 函数：timeout(1) 无法执行函数，而所有 docker 调用都需可被 timeout 包装
# （docker.exe 客户端传完即挂起的实证——见 STATUS R3）
DKDIR=$(mktemp -d "${TMPDIR:-/tmp}/recovery-dk-XXXXXX")
trap 'rm -rf "$DKDIR"' EXIT
printf '#!/usr/bin/env bash\nexport MSYS_NO_PATHCONV=1\nexec docker "$@"\n' > "$DKDIR/dk"
chmod +x "$DKDIR/dk"
PATH="$DKDIR:$PATH"

CID=b944a159ec468720c68dc9daf0824160c8ce6880d4498f932bafe81068e55d89
EXPECT_NAME=agentlab-demo-dev
if [ -n "${RECOVERY_SRC:-}" ]; then SRC="$RECOVERY_SRC"
elif [ -d /mnt/d/AgentLab/projects/demo ]; then SRC=/mnt/d/AgentLab/projects/demo
elif [ -d /d/AgentLab/projects/demo ]; then SRC=/d/AgentLab/projects/demo
else printf '阻断: 找不到宿主 bind 源（/mnt/d/… 与 /d/… 均不存在）\n' >&2; exit 1
fi
BASELINE="${RECOVERY_BASELINE:-57d5408cf2c6e80689d1adbc9f9ce8aa1a5796c0}"
MIN_WS="${RECOVERY_MIN_WS_FILES:-800}"
MIN_WT="${RECOVERY_MIN_WT_FILES:-300}"
TS=$(date +%Y%m%d-%H%M%S)
B="${RECOVERY_BACKUP_ROOT:-$HOME}/agentlab-recovery-$TS"
SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

# 强制前置门：未通过模拟验证不得对真实目标执行
if ! grep -q '^VALIDATION PASS' "$SCRIPT_DIR/validate-result.txt" 2>/dev/null; then
  printf '阻断: 未完成模拟验证——先运行 bash validate_recovery.sh "%s"\n' "$SCRIPT_DIR" >&2
  exit 1
fi

mkdir -p "$B"
LOG="$B/manifest.txt"
step(){ printf '[%s] %s\n' "$(date '+%F %T')" "$*" | tee -a "$LOG"; }
die(){ step "阻断（不重启）: $*"; exit 1; }
git_revparse(){ # $1=repo 路径 → stdout=HEAD hash 或 FAIL；错误全文留痕 ${GITERR:-/dev/null}（R3 诊断）
  local r="$1" h="" i
  for i in 1 2 3 4 5 6; do
    if h=$(git -C "$r" rev-parse HEAD 2>>"${GITERR:-/dev/null}"); then printf '%s' "$h"; return 0; fi
    sleep 2
  done
  printf 'FAIL'
}
# 容器内命令包装：命令成功执行才落 .ok 标记；读取失败不得被当作有效状态
snapexec(){ local out="$1"; shift
  # timeout 硬超时：docker.exe 客户端可能在容器侧进程退出后不返回（R3 实测）——以 .ok 标记与产物为准
  if timeout -k 15 60 "$@" >"$B/$out" 2>"$B/$out.err"; then : >"$B/$out.ok"; return 0; else rm -f "$B/$out.ok"; return 1; fi }

WS_EXCL=(--exclude='./target' --exclude='./.devcontainer/033-rollback'
         --exclude='.env' --exclude='.env.*'
         --exclude='./.container-state' --exclude='./.claude'
         --exclude='./.compound-engineering' --exclude='./.context'
         --exclude='./src/app/config'
         --exclude='*.pem' --exclude='*.key' --exclude='*.p12' --exclude='*.pfx')
WT_EXCL=(--exclude='*.pem' --exclude='*.key' --exclude='*.p12' --exclude='*.pfx'
         --exclude='.env' --exclude='.env.*')

step "== 0 容器核实 =="
step "写入协调（宿主执行者声明）: 两项任务 Agent 会话与宿主侧后台写入已停止；本脚本执行期间执行者不写 /workspace。写入控制主证据＝docker pause 冻结并经 inspect 证实；双读一致仅为辅助证据"
dk inspect -f 'ID={{.Id}} Name={{.Name}} Status={{.State.Status}} PidsLimit={{.HostConfig.PidsLimit}} Restart={{.HostConfig.RestartPolicy.Name}}' "$CID" | tee -a "$LOG"
NAME=$(dk inspect -f '{{.Name}}' "$CID"); NAME=${NAME#/}
if [ "$NAME" != "$EXPECT_NAME" ]; then step "提示: 实际名称($NAME)与预期($EXPECT_NAME)不同，以 ID 为锚点继续"; fi
STATUS0=$(dk inspect -f '{{.State.Status}}' "$CID")
WAS_PAUSED=0; FROZEN=0
if [ "$STATUS0" = paused ]; then WAS_PAUSED=1; FROZEN=1; step "检测到容器已 paused（人工预暂停）——冻结视为已生效，重启前将 unpause 并记录全过程"; fi
step "挂载清单（bind 源与 /tmp 属性以此输出为准）:"
dk inspect -f '{{range .Mounts}}{{.Type}}  {{.Source}}  ->  {{.Destination}}{{"\n"}}{{end}}' "$CID" | tee -a "$LOG"

# 挂载源确认（R2：删除"尾部相同即放行"；两条确认途径，均不成立即不重启）
WS_SRC=$(dk inspect -f '{{range .Mounts}}{{if eq .Destination "/workspace"}}{{.Source}}{{end}}{{end}}' "$CID")
norm_src(){ local s="${1%/}"
  if [[ "$s" =~ ^([A-Za-z]):[\\/](.*)$ ]]; then s="/mnt/${BASH_REMATCH[1],,}/${BASH_REMATCH[2]//\\//}"; fi
  printf '%s' "$s"; }
SRC_ACTUAL=$(norm_src "$WS_SRC")
step "/workspace bind 源: inspect 原值=$WS_SRC → 归一化=$SRC_ACTUAL；脚本 SRC=$SRC"
CONFIRM=none
CANARY=".canary-$TS-$RANDOM"
TOKEN=$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')
if printf '%s\n' "$TOKEN" > "$SRC/env-recovery-20261103/$CANARY" 2>/dev/null; then
  if timeout -k 15 60 dk cp "$CID:/workspace/env-recovery-20261103/$CANARY" - 2>>"$LOG" | tar -xO 2>>"$LOG" | grep -qxF "$TOKEN"; then
    CONFIRM=canary
  fi
  rm -f "$SRC/env-recovery-20261103/$CANARY"
fi
strip_mnt(){ local s="${1#/mnt/}"; printf '%s' "$s"; }
if [ "$CONFIRM" = none ] && { [ "${SRC_ACTUAL,,}" = "${SRC,,}" ] || [ "$(strip_mnt "${SRC_ACTUAL,,}")" = "$(strip_mnt "${SRC,,}")" ]; }; then CONFIRM=path-equal; fi
rm -f "$SRC/env-recovery-20261103/$CANARY" 2>/dev/null || true
step "挂载源确认方式=$CONFIRM"
[ "$CONFIRM" != none ] || die "无法确认实际挂载源与所备份目录对应（金丝雀与路径核对均未通过）——停止，不重启"
dk stats --no-stream --format '{{.PIDs}}' "$CID" >"$B/pids-pre.txt" 2>/dev/null || true
step "pids 采样① = $(cat "$B/pids-pre.txt" 2>/dev/null || echo 不可用)（重启前基线参考 23884；仅旁证，不作保全条件）"

step "== 0.5 提取语义自检（深层小归档：命名目录递归提取、排除生效、缺失成员退出码非 0）=="
ST="$B/.selftest"; rm -rf "$ST"; mkdir -p "$ST/pkg/a/b/c" "$ST/pkg/target" "$ST/out"
echo deep > "$ST/pkg/a/b/c/deep.txt"; echo top > "$ST/pkg/top.txt"; echo junk > "$ST/pkg/target/junk.bin"
tar -cf "$ST/arch.tar" -C "$ST" pkg
tar -xf "$ST/arch.tar" -C "$ST/out" 'pkg' --exclude='pkg/target' || die "自检: 命名目录提取失败"
[ -f "$ST/out/pkg/a/b/c/deep.txt" ] || die "自检: 深层文件未提取（递归提取不成立）"
[ "$(cat "$ST/out/pkg/a/b/c/deep.txt")" = deep ] || die "自检: 内容不符"
[ ! -e "$ST/out/pkg/target" ] || die "自检: 排除未生效"
if tar -xf "$ST/arch.tar" -C "$ST/out" 'pkg/不存在' 2>/dev/null; then die "自检: 缺失成员竟然成功（退出码语义不成立）"; fi
rm -rf "$ST"
step "自检通过（本机 tar 语义与脚本提取参数一致，R2-1 固化）"

step "== 1 前置状态快照（仅作变更登记；读取失败登记缺口，不作为保全条件）=="
if dk exec "$CID" true 2>/dev/null; then
  snapexec pre-head-workspace.txt dk exec "$CID" git -C /workspace rev-parse HEAD \
    || step "缺口: workspace pre-HEAD 读取失败（登记）"
  snapexec pre-status-workspace.txt dk exec "$CID" git -C /workspace status --porcelain=v1 -uall \
    || step "缺口: workspace pre-status 读取失败（登记）"
  snapexec pre-head-wt.txt dk exec "$CID" git -C /tmp/wt-038-f1 rev-parse HEAD \
    || step "缺口: wt pre-HEAD 读取失败（登记）"
  snapexec pre-status-wt.txt dk exec "$CID" git -C /tmp/wt-038-f1 status --porcelain=v1 -uall \
    || step "缺口: wt pre-status 读取失败（登记）"
  step "exec 通道可用；pre 快照落盘（含 .ok 标记者为有效）"
else
  step "exec 不可用（fork/EAGAIN）——/tmp 侧保全与一致性核验改用 docker cp（守护进程侧归档读取，不在容器内建进程）"
fi

step "== 1.5 写入控制：协调停止＋双读辅助证据＋pids 采样（默认不用 pause——R3 第 5/6 轮对照实证：pause 后本容器 exec/cp 通道即刻失效；RECOVERY_PAUSE=1 可显式启用）==="
if [ "$WAS_PAUSED" = 1 ]; then
  FROZEN=1
  step "容器已处于人工 paused——冻结视为已生效（inspect 确认在案），保全后解除"
elif [ "${RECOVERY_PAUSE:-0}" = 1 ]; then
  if dk pause "$CID" >>"$LOG" 2>&1; then
    PZ=$(dk inspect -f '{{.State.Status}}' "$CID" 2>/dev/null || echo unknown)
    if [ "$PZ" = paused ]; then
      FROZEN=1
      step "冻结已证实: Status=paused（inspect 确认）——此后全部读取在冻结态进行；双读一致自此刻起仅为辅助证据"
    else
      step "提示: pause 已执行但状态非 paused（$PZ）——冻结未证实，按『协调停止＋双读辅助证据』模式继续"
    fi
  else
    step "提示: docker pause 不可用（daemon 报错已录 manifest）——按『协调停止＋双读辅助证据＋pids 采样』模式继续；解除状态无变化（未误冻）"
  fi
else
  step "写入控制模式＝协调停止（宿主执行者声明）＋双读逐字节比对（辅助证据）＋pids 采样；不使用 pause（对照实证其使 exec/cp 失效）"
fi

step "== 2 保全 /tmp/wt-038-f1 与 /tmp/sec-test-038（overlay 可写层；两次独立捕获＝内容一致性证据）=="
capture_tree(){ # $1=rel $2=dest；stdout=通道名；提取侧统一落实敏感排除（R2：各备份通道均须排除）
  # 通道顺序（R3 两轮实测修正）：
  # ① exec 两步法——容器内 tar 源侧排除后落盘 /tmp/.recovery-capture.tgz，再单文件 docker cp 取出
  #   （exec 流式管道在 Windows Docker Desktop 上传完即挂起——20MB 完整落地后客户端不返回实证；
  #    静默 exec＋单文件 cp 均为本环境已验证可用形态）；
  # ② docker cp 目录流（无排除能力，wt 内 24GB 本地 target 会全量读取直至挂起——仅作 exec 不可用时兜底）；
  # ③ docker export 兜底（只含可写层，不含 bind 挂载）。
  local rel="$1" dest="$2" ch=""
  rm -rf "$dest"; mkdir -p "$dest"
  if [ "$(timeout -k 15 20 dk exec "$CID" true 2>/dev/null && echo ok || echo no)" = ok ]; then
    # 两步法（超时加固版）：容器内 tar 落盘 → 单文件 cp 取出；成功以「本地产物存在且可解」判定，
    # 不依赖 docker.exe 客户端退出码（传完即挂起实证——管道被它握住会让命令替换永不返回）
    timeout -k 15 45 dk exec "$CID" tar -C /tmp -czf /tmp/.recovery-capture.tgz --exclude="$rel/target" "$rel" >/dev/null 2>>"$LOG" || true
    WCAP=$(cygpath -m "$B/.recovery-capture.tgz" 2>/dev/null || echo "$B/.recovery-capture.tgz")
    timeout -k 15 45 dk cp "$CID:/tmp/.recovery-capture.tgz" "$WCAP" >>"$LOG" 2>&1 || true
    if [ -s "$B/.recovery-capture.tgz" ] && tar -xzf "$B/.recovery-capture.tgz" -C "$dest" --exclude="$rel/target" "${WT_EXCL[@]}" 2>>"$LOG"; then
      ch=exec-tar
    fi
    timeout -k 15 20 dk exec "$CID" rm -f /tmp/.recovery-capture.tgz >/dev/null 2>>"$LOG" || true
    rm -f "$B/.recovery-capture.tgz"
  fi
  if [ -z "$ch" ]; then
    if timeout -k 15 120 dk cp "$CID:/tmp/$rel" - 2>>"$LOG" | tar -x -C "$dest" --exclude="$rel/target" "${WT_EXCL[@]}"; then
      ch=docker-cp
    fi
  fi
  if [ -z "$ch" ]; then
    if timeout -k 15 180 dk export "$CID" 2>>"$LOG" | tar -x -C "$dest" --exclude="tmp/$rel/target" "${WT_EXCL[@]}" "tmp/$rel" \
       && [ -d "$dest/tmp/$rel" ]; then
      mv "$dest/tmp/$rel" "$dest/$rel"; rmdir "$dest/tmp" 2>/dev/null || true
      ch=docker-export
    fi
  fi
  if [ -n "$ch" ] && [ -d "$dest/$rel" ]; then printf '%s' "$ch"; return 0; fi
  rm -rf "$dest"; return 1
}

CH1=$(capture_tree wt-038-f1 "$B/wt-cap1") || die "三条通道（cp/exec/export）均未能保全 /tmp/wt-038-f1"
step "wt 首次捕获通道=$CH1"
( cd "$B/wt-cap1" && tar -czf "$B/wt-038-f1.tgz" wt-038-f1 ) || die "worktree 重打包失败"
tar -tzf "$B/wt-038-f1.tgz" > "$B/wt-038-f1.list" || die "worktree 备份不可读"
sha256sum "$B/wt-038-f1.tgz" | tee -a "$LOG"
if grep -nE '(^|/)\.env(\.|$)|\.(pem|key|p12|pfx)$|(^|/)wt-038-f1/target(/|$)' "$B/wt-038-f1.list" >"$B/wt-leak.txt"; then
  die "worktree 备份含应排除项（首行）: $(head -1 "$B/wt-leak.txt")"
fi
for f in wt-038-f1/.git \
         wt-038-f1/docs/security/reports/2026-11-02-038-f1-isolation-security.md \
         wt-038-f1/src/app/src/proc_session.rs; do
  grep -qF -- "$f" "$B/wt-038-f1.list" || die "worktree 备份缺关键项: $f"
done

CH2=$(capture_tree wt-038-f1 "$B/wt-cap2") || die "wt 二次捕获失败——无内容一致性证据，不重启"
step "wt 二次捕获通道=$CH2"
W1=$(find "$B/wt-cap1/wt-038-f1" | wc -l); W2=$(find "$B/wt-cap2/wt-038-f1" | wc -l)
step "wt 文件项计数: 首读=$W1 二读=$W2（下限 $MIN_WT，防空跑）"
[ "$W1" = "$W2" ] || die "wt 两次捕获文件项数不一致——存在写入活动"
[ "$W1" -ge "$MIN_WT" ] || die "wt 首读文件项数 $W1 低于下限 $MIN_WT——疑似空跑，人工核查"
if diff -r "$B/wt-cap1/wt-038-f1" "$B/wt-cap2/wt-038-f1" >"$B/wt-double-read-diff.txt" 2>&1; then
  step "PASS: wt 双读逐字节一致（含 .git 指针文件原文；备份窗口内容稳定）"
else
  step "wt 双读差异（首 20 行）:"; head -20 "$B/wt-double-read-diff.txt" | tee -a "$LOG" || true
  die "wt 双读内容差异——冻结态下不应发生，写入控制未证实，停止重启"
fi
rm -rf "$B/wt-cap2"

SEC1=$(capture_tree sec-test-038 "$B/sec-cap1") || {
  step "提示: sec-test-038 首次捕获失败——枚举 /tmp 其余内容（docker cp 流式清单，仅文件名）"
  dk cp "$CID:/tmp" - 2>/dev/null | tar -t 2>/dev/null | grep -vE '(^|/)wt-038-f1(/|$)' > "$B/tmp-others-list.txt" || true
  wc -l "$B/tmp-others-list.txt" | tee -a "$LOG"
  die "README §3 判定 /tmp/sec-test-038 必须保全而未取得（清单见 tmp-others-list.txt）；人工核实并调整判定后方可继续"
}
SEC2=$(capture_tree sec-test-038 "$B/sec-cap2") || die "sec-test-038 二次捕获失败——无内容一致性证据，不重启"
( cd "$B/sec-cap1" && tar -czf "$B/sec-test-038.tgz" sec-test-038 ) || die "sec-test-038 重打包失败"
tar -tzf "$B/sec-test-038.tgz" > "$B/sec-test-038.list" || die "sec-test-038 备份不可读"
sha256sum "$B/sec-test-038.tgz" | tee -a "$LOG"
S1=$(find "$B/sec-cap1/sec-test-038" | wc -l); S2=$(find "$B/sec-cap2/sec-test-038" | wc -l)
[ "$S1" = "$S2" ] && [ "$S1" -ge 1 ] || die "sec-test-038 两次捕获计数不一致或为空——存在写入活动/空跑"
if diff -r "$B/sec-cap1/sec-test-038" "$B/sec-cap2/sec-test-038" >"$B/sec-double-read-diff.txt" 2>&1; then
  step "PASS: sec-test-038 双读逐字节一致（$S1 项）"
else
  head -20 "$B/sec-double-read-diff.txt" | tee -a "$LOG" || true
  die "sec-test-038 双读内容差异——冻结态下不应发生，写入控制未证实，停止重启"
fi
rm -rf "$B/sec-cap2"

step "== 3 保全 /workspace（第一次读取；含 .git；宿主侧直读，不经容器进程）=="
step "排除（tar 流上排除，绝不先落盘完整包再删除凭据）: ./target、./.devcontainer/033-rollback（大体积）、.env/.env.*（任意深度；根目录 .env 实存含 1 个真实 API 密钥，值不记录）、./.container-state、./.claude、./.compound-engineering、./.context、./src/app/config、*.pem/*.key/*.p12/*.pfx（凭据扩展名信号；已跟踪测试夹具另按 2d 独立保全当前内容与删除状态）"
tar -C "$SRC" "${WS_EXCL[@]}" -czf "$B/workspace-full.tgz" . 2>"$B/tar-workspace.err" \
  || die "workspace tar 失败: $(tail -3 "$B/tar-workspace.err")"
sha256sum "$B/workspace-full.tgz" | tee -a "$LOG"
tar -tzf "$B/workspace-full.tgz" > "$B/workspace-full.list" || die "workspace 备份不可读（tar -t 失败）"
if grep -nE '(^|/)\.env(\.|$)|\.(pem|key|p12|pfx)$|^\.(/(target|\.devcontainer/033-rollback|\.container-state|\.claude|\.compound-engineering|\.context|src/app/config))(/|$)' "$B/workspace-full.list" >"$B/leak-scan.txt"; then
  die "备份清单出现应排除项（首行）: $(head -1 "$B/leak-scan.txt")（明细 leak-scan.txt）"
fi
step "排除泄漏断言通过（无 .env*／凭据扩展名／排除目录残留）"
for f in ./.git/objects ./.git/worktrees/wt-038-f1/HEAD ./docs/specs/038-github-actions-ci.md ./env-recovery-20261103/recover.sh ./env-recovery-20261103/verify.sh; do
  grep -qF -- "$f" "$B/workspace-full.list" || die "workspace 备份缺关键项: $f"
done

step "== 3a 备份副本核验（HEAD、对象库 fsck——只读副本，不触碰原仓库）=="
rm -rf "$B/ws-check"; mkdir -p "$B/ws-check"
tar -xzf "$B/workspace-full.tgz" -C "$B/ws-check" || die "解包 workspace 备份失败"
GITERR="$B/git-revparse.err"; : > "$GITERR"
HEAD_HASH=$(git_revparse "$B/ws-check")
step "备份副本 HEAD=$HEAD_HASH（预期 $BASELINE）"
[ "$HEAD_HASH" = "$BASELINE" ] || die "备份副本 HEAD 与预期不符"
FSCK_OK=0
for _fsck_try in 1 2 3; do
  if git -C "$B/ws-check" fsck --full >"$B/fsck-workspace-backup.txt" 2>&1; then FSCK_OK=1; break; fi
  sleep 3   # 新建解包树可能被 Windows 扫描器短暂锁文件——退避重试（R3 实测瞬态 FAIL）
done
if [ "$FSCK_OK" = 1 ]; then
  step "备份副本 git fsck --full 通过（dangling 提示不算失败，明细见 fsck-workspace-backup.txt）"
else
  die "备份对象库 fsck 失败，见 $B/fsck-workspace-backup.txt"
fi

step "== 3b workspace 内容双读核验（保全条件主体：同一源两次独立读取逐字节比对）=="
tar -C "$SRC" "${WS_EXCL[@]}" -cf "$B/workspace-second.tar" . 2>>"$LOG" || die "workspace 二次读取失败"
rm -rf "$B/ws-check2"; mkdir -p "$B/ws-check2"
tar -xf "$B/workspace-second.tar" -C "$B/ws-check2" || die "二次读取解包失败"
N1=$(find "$B/ws-check" | wc -l); N2=$(find "$B/ws-check2" | wc -l)
step "文件项计数: 首读=$N1 二读=$N2（下限 $MIN_WS，防空跑）"
[ "$N1" = "$N2" ] || die "两次读取文件项数不一致——存在写入活动"
[ "$N1" -ge "$MIN_WS" ] || die "首读文件项数 $N1 低于下限 $MIN_WS——疑似空跑，人工核查"
if diff -r "$B/ws-check" "$B/ws-check2" >"$B/ws-double-read-diff.txt" 2>&1; then
  step "PASS: workspace 双读逐字节一致（备份窗口内容稳定）"
else
  step "workspace 双读差异（首 20 行）:"; head -20 "$B/ws-double-read-diff.txt" | tee -a "$LOG" || true
  die "workspace 双读内容差异——冻结态下不应发生，写入控制未证实，停止重启"
fi
rm -f "$B/workspace-second.tar"

step "== 3c 非敏感测试夹具：当前内容与删除状态独立保全（不回填副本、不宣称副本＝原工作目录备份）=="
git -C "$B/ws-check" ls-files -- '*.pem' '*.key' '*.p12' '*.pfx' 'src/app/tests/fixtures/*' \
  > "$B/tracked-excluded.txt" 2>>"$LOG" || true
git -C "$B/ws-check" ls-files -- '.env' '.env.*' '.container-state/*' '.claude/*' \
  '.compound-engineering/*' '.context/*' 'src/app/config/*' > "$B/tracked-review.txt" 2>>"$LOG" || true
: > "$B/fixtures-status.txt"; : > "$B/fixtures-present.txt"
while IFS= read -r fx; do
  [ -z "$fx" ] && continue
  if [ -f "$SRC/$fx" ]; then printf 'present %s\n' "$fx" >> "$B/fixtures-status.txt"; printf '%s\n' "$fx" >> "$B/fixtures-present.txt"
  else printf 'deleted %s\n' "$fx" >> "$B/fixtures-status.txt"; fi
done < "$B/tracked-excluded.txt"
if [ -s "$B/fixtures-present.txt" ]; then
  mapfile -t FIXARR < "$B/fixtures-present.txt"
  tar -C "$SRC" -czf "$B/fixtures-current.tgz" "${FIXARR[@]}" || die "夹具当前内容保全失败"
  tar -tzf "$B/fixtures-current.tgz" > "$B/fixtures-current.list" || die "夹具备份不可读"
  if grep -nE '(^|/)\.env(\.|$)|(^|/)\.(container-state|claude|compound-engineering|context)(/|$)|(^|/)src/app/config(/|$)' "$B/fixtures-current.list" >"$B/fixtures-leak.txt"; then
    die "夹具备份含凭据类路径（首行）: $(head -1 "$B/fixtures-leak.txt")"
  fi
  rm -rf "$B/fx-check"; mkdir -p "$B/fx-check"
  tar -xzf "$B/fixtures-current.tgz" -C "$B/fx-check" || die "夹具备份解包失败"
  while IFS= read -r fx; do
    cmp -s "$SRC/$fx" "$B/fx-check/$fx" || die "夹具内容复核不符: $fx"
  done < "$B/fixtures-present.txt"
  rm -rf "$B/fx-check"
  step "夹具当前内容已独立保全并逐字节复核（fixtures-current.tgz，$(grep -c . "$B/fixtures-current.list") 项）"
else
  step "夹具: 无 present 项（无内容需保全）"
fi
step "夹具删除状态登记: $(grep -c '^deleted' "$B/fixtures-status.txt" || true) 项 deleted ／ $(grep -c '^present' "$B/fixtures-status.txt" || true) 项 present（全表 fixtures-status.txt）"
while IFS= read -r rv; do
  [ -z "$rv" ] && continue
  step "须人工判断的已跟踪排除项（未自动保全）: $rv"
done < "$B/tracked-review.txt"

step "== 3d 备份副本 status（登记备份内容；被排除的已跟踪文件显示 ' D' 属排除策略真实结果，非原工作目录删除状态）=="
if git -C "$B/ws-check" status --porcelain=v1 -uall >"$B/status-ws-copy.txt" 2>"$B/status-ws-copy.err"; then
  : > "$B/status-ws-copy.ok"
else
  rm -f "$B/status-ws-copy.ok"; step "缺口: 备份副本 status 读取失败（登记）"
fi
step "workspace 备份内容状态共 $(grep -c . "$B/status-ws-copy.txt" || true) 条（全文 status-ws-copy.txt）"

step "== 3e worktree 备份副本状态核验（副本两侧指针互相重定向；只读副本，不误查原仓库）=="
rm -rf "$B/wt-check"; mkdir -p "$B/wt-check"
tar -xzf "$B/wt-038-f1.tgz" -C "$B/wt-check" || die "解包 wt-038-f1 失败"
GITFILE=$(cat "$B/wt-check/wt-038-f1/.git")
step "worktree .git 指针原文（仅记录，不据此查原仓库）: $GITFILE"
case "$GITFILE" in
  gitdir:*)
    # 副本自足化：worktree 指针与元数据回指都改指备份副本内路径。
    # Windows 的 git 要求指针为原生形式（C:/…）——POSIX 形式实测报
    # "gitfile does not point to a valid repository"，故 cygpath -m 转换；
    # WSL/Linux 无 cygpath，POSIX 路径原样即可
    if command -v cygpath >/dev/null 2>&1; then
      WTG1=$(cygpath -m "$B/ws-check/.git/worktrees/wt-038-f1")
      WTG2=$(cygpath -m "$B/wt-check/wt-038-f1/.git")
    else
      WTG1="$B/ws-check/.git/worktrees/wt-038-f1"
      WTG2="$B/wt-check/wt-038-f1/.git"
    fi
    printf 'gitdir: %s\n' "$WTG1" > "$B/wt-check/wt-038-f1/.git"
    printf '%s\n' "$WTG2" > "$B/ws-check/.git/worktrees/wt-038-f1/gitdir"
    ;;
  *) die "wt-038-f1/.git 不是 worktree 指针文件: $GITFILE";;
esac
WT_HEAD=$(git_revparse "$B/wt-check/wt-038-f1")
step "worktree 备份副本 HEAD=$WT_HEAD（预期 $BASELINE）"
[ "$WT_HEAD" = "$BASELINE" ] || die "worktree 备份副本 HEAD 与预期不符"
if git -C "$B/wt-check/wt-038-f1" status --porcelain=v1 -uall >"$B/status-wt-copy.txt" 2>"$B/status-wt-copy.err"; then
  : > "$B/status-wt-copy.ok"
else
  rm -f "$B/status-wt-copy.ok"; step "缺口: 备份副本 wt status 读取失败（登记）"
fi
step "worktree 备份内容状态共 $(grep -c . "$B/status-wt-copy.txt" || true) 条（登记用，权威清单全文 status-wt-copy.txt）:"
tee -a "$LOG" < "$B/status-wt-copy.txt"
grep -qF 'docs/specs/038-github-actions-ci.md' "$B/status-wt-copy.txt" \
  || step "提示: 副本 status 未含规格文件——与容器内实证（worktree 版含迭代 37 段）不符时须人工核查"
step "参考清单交叉核对（安全报告快照仅补充，权威＝status-wt-copy.txt）:"
while IFS= read -r ref; do
  [ -z "$ref" ] && continue
  if grep -qF -- "$ref" "$B/status-wt-copy.txt"; then step "  命中: $ref"; else step "  未命中（登记差异，以 status 为准）: $ref"; fi
done <<'EOF'
src/app/src/apply_patch.rs
src/app/src/mcp/transport.rs
src/app/src/proc_session.rs
src/app/src/sandbox.rs
src/app/src/view_image.rs
src/app/tests/memory_storage_files.rs
src/app/tests/request_permissions_sandbox.rs
src/app/tests/sandbox_lifecycle.rs
docs/specs/031-request-permissions.md
docs/specs/033-tool-sandbox.md
docs/security/reports/2026-11-02-038-f1-isolation-security.md
EOF

step "== 4 一致性汇总（保全条件已在 3b/2 以内容双读承载；此处登记旁证与状态差异）=="
dk stats --no-stream --format '{{.PIDs}}' "$CID" >"$B/pids-post.txt" 2>/dev/null || true
step "pids 采样② = $(cat "$B/pids-post.txt" 2>/dev/null || echo 不可用)（采样① = $(cat "$B/pids-pre.txt" 2>/dev/null || echo 不可用)；仅旁证）"
if [ -f "$B/pre-status-wt.txt.ok" ] && dk exec "$CID" true 2>/dev/null; then
  if snapexec post-status-wt.txt dk exec "$CID" git -C /tmp/wt-038-f1 status --porcelain=v1 -uall; then
    if diff -q "$B/pre-status-wt.txt" "$B/post-status-wt.txt" >/dev/null; then
      step "登记: wt pre/post status 一致（git status 仅登记变更，非保全条件）"
    else
      step "登记: wt pre/post status 存在差异（git status 仅登记变更，非保全条件；内容双读已通过）:"
      diff "$B/pre-status-wt.txt" "$B/post-status-wt.txt" | tee -a "$LOG" || true
    fi
  fi
fi
if [ "$FROZEN" = 1 ]; then EVID="冻结证实（Status=paused，inspect 确认——主证据）＋三组双读逐字节一致（辅助）"; else EVID="协调停止（宿主执行者声明）＋三组双读逐字节一致（辅助证据，pause 不可用）＋pids 采样"; fi
step "【里程碑：备份完成】备份目录=$B；保全条件证据=$EVID＋文件项计数、fsck-workspace-backup.txt、HEAD 双核验、泄漏断言（leak-scan/wt-leak/fixtures-leak 均空）、fixtures-current.tgz＋fixtures-status.txt；登记类=status-*-copy.txt、pids-*.txt、tracked-excluded/review.txt"

if [ "${RECOVERY_SKIP_RESTART:-0}" = 1 ]; then
  # 环境已恢复（exec 正常）时的保全模式：只保全+核验，不消耗原地重启授权（R3 指示 4）
  if [ "$FROZEN" = 1 ]; then
    if ! dk unpause "$CID" 2>>"$LOG"; then die "unpause 失败——保留现场（容器仍冻结，可手动 unpause）"; fi
    UP=$(dk inspect -f '{{.State.Status}}' "$CID" 2>/dev/null || echo unknown)
    [ "$UP" = running ] || die "unpause 后状态非 running（$UP）——保留现场"
    step "解除冻结已证实: Status=running（跳过重启路径）"
  fi
  step "【保全完成（按指示跳过自动重启）】备份目录=$B；容器保持运行、原地重启授权未消耗；核验: bash \"$SCRIPT_DIR/verify.sh\" \"$B\""
  exit 0
fi
step "== 5 原地重启一次（既有授权；有限等待 60s；仅 restart，不删除/不重建）=="
if [ "$FROZEN" != 1 ]; then step "提示: runc 侧 pthread_create EAGAIN（pause 实证）——restart 走同一 runc 通道，存在同类失败可能；失败则保留现场并按人工指引处理"; fi
if [ "$FROZEN" = 1 ]; then
  step "解除冻结开始: $(date '+%F %T')"
  if ! dk unpause "$CID" 2>>"$LOG"; then die "unpause 失败——保留现场（可手动 dk unpause 后再重启），不重启"; fi
  UP=$(dk inspect -f '{{.State.Status}}' "$CID" 2>/dev/null || echo unknown)
  [ "$UP" = running ] || die "unpause 已执行但状态非 running（$UP）——保留现场，不重启"
  step "解除冻结已证实: Status=running（$(date '+%F %T')）→ 立即重启"
fi
step "重启前 ID=$(dk inspect -f '{{.Id}}' "$CID")"
if ! dk restart -t 60 "$CID" | tee -a "$LOG"; then die "docker restart 失败——保留现场，不自动重试"; fi
sleep 2
if ! AFTER=$(dk inspect -f '{{.Id}} {{.State.Status}}' "$CID" 2>>"$LOG"); then
  die "重启后状态查询失败——不登记重启完成，保留现场"
fi
step "重启后: $AFTER"
if [ "${AFTER%% *}" != "$CID" ]; then die "重启已执行但容器 ID 变化（$AFTER）——不登记重启完成，保留现场"; fi
if [ "${AFTER##* }" != running ]; then die "重启已执行但容器未运行（$AFTER）——不登记重启完成，保留现场"; fi
step "【里程碑：重启完成】ID 未变且 Status=running"
step "下一步: bash \"$SCRIPT_DIR/verify.sh\" \"$B\"（显式传入本次备份目录；核验未全过将退出码非 0）"
