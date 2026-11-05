#!/usr/bin/env bash
# 重启后恢复核验（R2 修订）。用法: bash verify.sh <备份目录>（显式传入，不自动选最新）。
# 判定口径（R2）：检查命令失败＝FAIL；必要检查缺失＝FAIL（不得判定核验完成）；
#   成功执行且输出为空的 git status 是有效结果；核验未全部通过 → 最终退出码非 0。
# 本脚本仅作重启后回归检测，不补充 recover.sh 的重启前保全条件。
# 仅限模拟验证使用的覆盖项：RECOVERY_BASELINE。
set -uo pipefail

B="${1:-}"
if [ -z "$B" ] || [ ! -d "$B" ]; then printf '用法: verify.sh <备份目录>\n' >&2; exit 2; fi
B=$(cd "$B" && pwd)
# 仅 docker.exe 需要 MSYS 路径改写屏蔽；git.exe 依赖 MSYS 路径解析——绝不全局 export（R3 修复）。
# dk 为真实脚本（timeout 无法包装 bash 函数，而 docker.exe 客户端需要超时防护）
DKDIR=$(mktemp -d "${TMPDIR:-/tmp}/recovery-dk-XXXXXX")
trap 'rm -rf "$DKDIR"' EXIT
printf '#!/usr/bin/env bash\nexport MSYS_NO_PATHCONV=1\nexec docker "$@"\n' > "$DKDIR/dk"
chmod +x "$DKDIR/dk"
PATH="$DKDIR:$PATH"
CID=b944a159ec468720c68dc9daf0824160c8ce6880d4498f932bafe81068e55d89
BASELINE="${RECOVERY_BASELINE:-57d5408cf2c6e80689d1adbc9f9ce8aa1a5796c0}"
MIN_WS="${RECOVERY_MIN_WS_FILES:-800}"
MIN_WT="${RECOVERY_MIN_WT_FILES:-300}"
if [ -n "${RECOVERY_SRC:-}" ]; then SRC="$RECOVERY_SRC"
elif [ -d /mnt/d/AgentLab/projects/demo ]; then SRC=/mnt/d/AgentLab/projects/demo
elif [ -d /d/AgentLab/projects/demo ]; then SRC=/d/AgentLab/projects/demo
else SRC=""
fi
V="$B/post-restart-verify.txt"
if [ -n "${RECOVERY_ARCHIVE_DEST:-}" ]; then DEST="$RECOVERY_ARCHIVE_DEST"
elif [ -n "$SRC" ]; then DEST="$SRC/docs/logs/2026-11-03-env-recovery"
else DEST=""
fi
WT_EXCL=(--exclude='*.pem' --exclude='*.key' --exclude='*.p12' --exclude='*.pfx'
         --exclude='.env' --exclude='.env.*')
# 内容回归的哈希行集：剪除口径与备份排除一致（.git 由 HEAD+fsck 单独核验，不入本集）
tree_hash(){ ( cd "$1" 2>/dev/null || exit 1
  find . \
    \( -type d \( -name .git -o -name target -o -name .container-state -o -name .claude \
         -o -name .compound-engineering -o -name .context -o -name 033-rollback \
         -o -name .env -o -name '.env.*' -o -path './src/app/config' \) -prune \) -o \
    \( -type f ! -name '*.pem' ! -name '*.key' ! -name '*.p12' ! -name '*.pfx' \
         ! -name '.env' ! -name '.env.*' -print0 \) \
    | LC_ALL=C sort -z | xargs -0 -r sha256sum | LC_ALL=C sort ) ; }
say(){ printf '[%s] %s\n' "$(date '+%F %T')" "$*" | tee -a "$V"; }
FAILS=0
fail(){ say "FAIL: $*"; FAILS=$((FAILS+1)); }
# 命令成功执行才落 .ok 标记：空输出＋.ok＝有效（如干净树的空 status）；失败＝无标记＝必要检查缺失
snap(){ local out="$1"; shift
  # timeout 硬超时：docker.exe 客户端挂起不应拖死核验——失败按必要检查缺失/命令失败口径如实登记
  if timeout -k 15 120 "$@" >"$B/$out" 2>"$B/$out.err"; then : >"$B/$out.ok"; return 0; else rm -f "$B/$out.ok"; return 1; fi }

say "== 0 容器状态（必要）=="
if snap ctlin dk inspect -f 'ID={{.Id}} Name={{.Name}} Status={{.State.Status}} StartedAt={{.State.StartedAt}} RestartCount={{.RestartCount}}' "$CID"; then
  cat "$B/ctlin" | tee -a "$V"
  grep -q "ID=$CID" "$B/ctlin" || fail "容器 ID 与备份脚本锚点不一致"
  grep -q 'Status=running' "$B/ctlin" || fail "容器未处于 running 状态"
else
  fail "docker inspect 失败（必要检查缺失）"
fi

say "== 1 最小 shell 命令（必要）=="
if snap shellout dk exec "$CID" sh -lc 'echo shell-ok' && grep -q 'shell-ok' "$B/shellout"; then
  say "PASS: exec shell 可用"
else
  fail "docker exec 不可用或输出异常"
fi

say "== 2 进程数量与 pids 配额（旁证，不要求归零）=="
timeout -k 15 60 dk exec "$CID" sh -c 'echo "pids.max=$(cat /sys/fs/cgroup/pids.max 2>/dev/null || cat /sys/fs/cgroup/pids/pids.max 2>/dev/null)"; echo "pids.current=$(cat /sys/fs/cgroup/pids.current 2>/dev/null || cat /sys/fs/cgroup/pids/pids.current 2>/dev/null)"; echo "进程总数=$(ps -e --no-headers 2>/dev/null | wc -l)"' 2>&1 | tee -a "$V" || say "提示: pids/进程读取失败（旁证项，不影响判定）"
say "对照: 重启前 pids.current=23884、进程总数=24268；备份期采样 pre=$(cat "$B/pids-pre.txt" 2>/dev/null || echo 无)/post=$(cat "$B/pids-post.txt" 2>/dev/null || echo 无)"

say "== 3 两工作树 HEAD/status 与保全前及备份副本比对（必要）=="
if snap post-head-workspace.txt dk exec "$CID" git -C /workspace rev-parse HEAD; then
  h=$(cat "$B/post-head-workspace.txt")
  if [ "$h" = "$BASELINE" ]; then say "PASS: workspace HEAD = 基线"
  else fail "workspace HEAD = $h ≠ 基线"; fi
  if [ -f "$B/pre-head-workspace.txt.ok" ]; then
    diff -q "$B/pre-head-workspace.txt" "$B/post-head-workspace.txt" >/dev/null 2>&1 \
      && say "PASS: workspace HEAD 与保全前一致" || fail "workspace HEAD 与保全前不一致"
  fi
else
  fail "workspace HEAD 读取失败（必要检查缺失）"
fi
if snap post-status-workspace.txt dk exec "$CID" git -C /workspace status --porcelain=v1 -uall; then
  if [ -f "$B/pre-status-workspace.txt.ok" ]; then
    if diff -q "$B/pre-status-workspace.txt" "$B/post-status-workspace.txt" >/dev/null 2>&1; then
      say "PASS: workspace status 与保全前一致（$(grep -c . "$B/post-status-workspace.txt") 条）"
    else
      fail "workspace status 与保全前不一致:"
      diff "$B/pre-status-workspace.txt" "$B/post-status-workspace.txt" | tee -a "$V" || true
    fi
  elif [ -f "$B/status-ws-copy.ok" ]; then
    if diff -q "$B/status-ws-copy.txt" "$B/post-status-workspace.txt" >/dev/null 2>&1; then
      say "PASS: workspace status 与备份副本一致（$(grep -c . "$B/post-status-workspace.txt") 条）——重启后回归检测"
    else
      fail "workspace status 与备份副本不一致:"
      diff "$B/status-ws-copy.txt" "$B/post-status-workspace.txt" | tee -a "$V" || true
    fi
  else
    fail "workspace status 无可比对的保全基准（pre 与副本均缺失/读取失败）——必要检查缺失"
  fi
else
  fail "workspace status 读取失败（必要检查缺失）"
fi
if snap post-head-wt.txt dk exec "$CID" git -C /tmp/wt-038-f1 rev-parse HEAD; then
  h=$(cat "$B/post-head-wt.txt")
  if [ "$h" = "$BASELINE" ]; then say "PASS: wt HEAD = 基线"
  else fail "wt HEAD = $h ≠ 基线"; fi
  if [ -f "$B/pre-head-wt.txt.ok" ]; then
    diff -q "$B/pre-head-wt.txt" "$B/post-head-wt.txt" >/dev/null 2>&1 \
      && say "PASS: wt HEAD 与保全前一致" || fail "wt HEAD 与保全前不一致"
  fi
else
  fail "wt HEAD 读取失败（必要检查缺失）"
fi
if snap post-status-wt.txt dk exec "$CID" git -C /tmp/wt-038-f1 status --porcelain=v1 -uall; then
  if [ -f "$B/pre-status-wt.txt.ok" ]; then
    if diff -q "$B/pre-status-wt.txt" "$B/post-status-wt.txt" >/dev/null 2>&1; then
      say "PASS: wt status 与保全前一致（$(grep -c . "$B/post-status-wt.txt") 条）"
    else
      fail "wt status 与保全前不一致:"
      diff "$B/pre-status-wt.txt" "$B/post-status-wt.txt" | tee -a "$V" || true
    fi
  elif [ -f "$B/status-wt-copy.ok" ]; then
    if diff -q "$B/status-wt-copy.txt" "$B/post-status-wt.txt" >/dev/null 2>&1; then
      say "PASS: wt status 与备份副本一致（$(grep -c . "$B/post-status-wt.txt") 条）——重启后回归检测"
    else
      fail "wt status 与备份副本不一致:"
      diff "$B/status-wt-copy.txt" "$B/post-status-wt.txt" | tee -a "$V" || true
    fi
  else
    fail "wt status 无可比对的保全基准（pre 与副本均缺失/读取失败）——必要检查缺失"
  fi
else
  fail "wt status 读取失败（必要检查缺失）"
fi
say "任务 B 候选抽查（必要）:"
if snap spot-spec dk exec "$CID" sh -c 'grep -c "迭代 37" /tmp/wt-038-f1/docs/specs/038-github-actions-ci.md' \
   && [ "$(cat "$B/spot-spec")" -ge 1 ] 2>/dev/null; then
  say "PASS: 规格含迭代 37 段（$(cat "$B/spot-spec") 处）"
else
  fail "规格迭代 37 段抽查失败"
fi
if snap spot-report dk exec "$CID" sh -c 'ls /tmp/wt-038-f1/docs/security/reports/2026-11-02-038-f1-isolation-security.md'; then
  say "PASS: 安全报告在位"
else
  fail "安全报告缺失"
fi
if snap spot-sec dk exec "$CID" sh -c 'test -d /tmp/sec-test-038'; then
  say "PASS: /tmp/sec-test-038 在位"
else
  fail "/tmp/sec-test-038 缺失"
fi

say "== 3.5 内容回归核验（必要）：live 与备份逐哈希比对——M 状态文件内容变化必检，不只看 status 行 =="
if [ -z "$SRC" ]; then
  fail "找不到宿主 bind 源——workspace 内容回归无法执行（必要检查缺失）"
elif [ ! -d "$B/ws-check" ] || [ ! -d "$B/wt-cap1/wt-038-f1" ]; then
  fail "备份副本树缺失（ws-check / wt-cap1）——必要检查缺失"
else
  tree_hash "$SRC" > "$B/live-ws.sha" || true
  tree_hash "$B/ws-check" > "$B/copy-ws.sha" || true
  NW=$(grep -c . "$B/live-ws.sha" 2>/dev/null || echo 0); case "$NW" in ''|*[!0-9]*) NW=0;; esac
  if [ "$NW" -lt "$MIN_WS" ]; then
    fail "workspace live 哈希项数 $NW 低于下限 $MIN_WS——疑似空跑，人工核查"
  elif diff "$B/live-ws.sha" "$B/copy-ws.sha" >"$B/ws-content-regression.diff" 2>&1; then
    say "PASS: workspace live 与备份逐哈希一致（$NW 项）"
  else
    fail "workspace live 与备份内容差异（首 20 行）:"
    head -20 "$B/ws-content-regression.diff" | tee -a "$V" || true
  fi
  rm -rf "$B/wt-live"; mkdir -p "$B/wt-live"
  # 两步法（超时加固）：容器内 tar 落盘 → 单文件 cp 取出（exec 流式管道会挂起，R3 实测）
  timeout -k 15 45 dk exec "$CID" tar -C /tmp -czf /tmp/.recovery-verify-capture.tgz --exclude='wt-038-f1/target' wt-038-f1 >/dev/null 2>>"$V" || true
  WVCP=$(cygpath -m "$B/.recovery-verify-capture.tgz" 2>/dev/null || echo "$B/.recovery-verify-capture.tgz")
  timeout -k 15 45 dk cp "$CID:/tmp/.recovery-verify-capture.tgz" "$WVCP" >>"$V" 2>&1 || true
  if [ -s "$B/.recovery-verify-capture.tgz" ] && tar -xzf "$B/.recovery-verify-capture.tgz" -C "$B/wt-live" --exclude='wt-038-f1/target' "${WT_EXCL[@]}"; then
    tree_hash "$B/wt-live/wt-038-f1" > "$B/live-wt.sha" || true
    tree_hash "$B/wt-cap1/wt-038-f1" > "$B/copy-wt.sha" || true
    NW2=$(grep -c . "$B/live-wt.sha" 2>/dev/null || echo 0); case "$NW2" in ''|*[!0-9]*) NW2=0;; esac
    if [ "$NW2" -lt "$MIN_WT" ]; then
      fail "wt live 哈希项数 $NW2 低于下限 $MIN_WT——疑似空跑，人工核查"
    elif diff "$B/live-wt.sha" "$B/copy-wt.sha" >"$B/wt-content-regression.diff" 2>&1; then
      say "PASS: wt live 与备份逐哈希一致（$NW2 项）"
    else
      fail "wt live 与备份内容差异（首 20 行）:"
      head -20 "$B/wt-content-regression.diff" | tee -a "$V" || true
    fi
  else
    fail "wt live 取出失败（必要检查缺失）"
  fi
  timeout -k 15 20 dk exec "$CID" rm -f /tmp/.recovery-verify-capture.tgz >/dev/null 2>>"$V" || true
  rm -f "$B/.recovery-verify-capture.tgz"
fi

say "== 4 Git 对象完整性（必要；覆盖共享对象库与两工作树引用）=="
if snap fsck-post dk exec "$CID" git -C /workspace fsck --full; then
  say "PASS: git fsck --full 通过（明细 $B/fsck-post.txt）"
else
  fail "fsck 失败，末尾 20 行:"
  tail -20 "$B/fsck-post.txt" 2>/dev/null | tee -a "$V" || true
fi

say "== 5 夹具保全证据与活环境一致性（必要）=="
if [ ! -f "$B/tracked-excluded.txt" ] || [ ! -f "$B/fixtures-status.txt" ]; then
  fail "夹具保全证据缺失（tracked-excluded.txt / fixtures-status.txt）"
else
  PRESENT_N=0
  while IFS= read -r line; do
    case "$line" in
      present*)
        p=${line#present }; PRESENT_N=$((PRESENT_N+1))
        grep -qF -- "$p" "$B/fixtures-current.list" 2>/dev/null \
          || fail "present 夹具不在 fixtures-current.tgz 内: $p"
        if snap "fx-live-$PRESENT_N" dk exec "$CID" test -f "/workspace/$p"; then
          say "PASS: 夹具活环境在位: $p"
        else
          fail "夹具活环境缺失: $p"
        fi ;;
      deleted*)
        p=${line#deleted }
        if snap "fxd-live-$PRESENT_N" dk exec "$CID" test -f "/workspace/$p"; then
          fail "登记为 deleted 但活环境仍存在（状态矛盾）: $p"
        else
          say "PASS: 夹具删除状态与活环境一致: $p"
        fi ;;
    esac
  done < "$B/fixtures-status.txt"
  [ "$PRESENT_N" = 0 ] || { [ -f "$B/fixtures-current.tgz" ] || fail "fixtures-current.tgz 缺失"; }
  say "夹具登记共 $(grep -c . "$B/fixtures-status.txt") 项"
fi

say "== 6 执行器自动拉起检查（旁证）=="
dk inspect -f 'RestartPolicy={{.HostConfig.RestartPolicy.Name}}' "$CID" 2>&1 | tee -a "$V" || true
timeout -k 15 60 dk exec "$CID" sh -c 'ps -ef --sort=start_time | tail -30' 2>&1 | tee -a "$V" || say "提示: 进程列举失败（旁证项）"
say "判读口径: 重启后仅应见 PID1(entrypoint) 与本核验 shell；若出现 agent 执行器被拉起，记录其 PID/启动时间用于交接，禁止按模糊进程名批量杀共享进程"

say "== 7 备份目录完整性（必要）=="
[ -f "$B/manifest.txt" ] || fail "manifest.txt 缺失（备份目录不完整）"
for f in workspace-full.tgz wt-038-f1.tgz sec-test-038.tgz; do
  [ -s "$B/$f" ] || fail "备份产物缺失或为空: $f"
done
[ -d "$B/ws-check" ] || fail "备份副本树缺失: ws-check"
[ -d "$B/wt-cap1/wt-038-f1" ] || fail "备份副本树缺失: wt-cap1/wt-038-f1"

say "== 里程碑判定 =="
if [ "$FAILS" -eq 0 ]; then
  say "【里程碑：核验完成】全部必要检查通过；证据=post-restart-verify.txt 全文"
  RC=0
else
  say "【里程碑：核验未完成】FAIL=$FAILS 项（含检查命令失败与必要检查缺失）——保留现场，勿清理备份目录"
  RC=1
fi

say "== 8 证据归档（归档自身会在 workspace 新增 docs/logs/2026-11-03-env-recovery/ 条目，预期现象非回归）=="
mkdir -p "$DEST" 2>>"$V" || true
cp -f "$B/manifest.txt" "$B/post-restart-verify.txt" "$B"/fsck-*.txt "$B"/status-*.txt "$B"/*.list \
      "$B"/tracked-excluded.txt "$B"/tracked-review.txt "$B"/fixtures-status.txt "$B"/fixtures-present.txt \
      "$B"/pids-*.txt "$B"/*double-read-diff.txt "$B"/*-leak.txt \
      "$B"/*.sha "$B"/*content-regression.diff \
      "$B"/pre-*.txt "$B"/pre-*.ok "$B"/post-*.txt "$B"/tar-workspace.err "$B"/tmp-others-list.txt \
      "$DEST"/ 2>>"$V" || say "提示: 部分证据归档失败（明细已入 $V）；备份目录本体仍在 $B"
say "归档目标: $DEST（本行与归档结果写在归档副本之后，归档内以此前内容为准）"
exit "$RC"
