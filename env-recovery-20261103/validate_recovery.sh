#!/usr/bin/env bash
# 恢复脚本模拟验证（R2）。全部在 mktemp 临时目录＋模拟 docker（shim）内进行：
# 不触碰真实容器、真实 /workspace、真实 $HOME——这正是「不得对真实目标试运行整套恢复流程」的落实。
# 用法: bash validate_recovery.sh <env-recovery 目录>
# 产出: <env-recovery>/validate-summary.txt（全程证据）；全部通过时另写 validate-result.txt（含 VALIDATION PASS，
#       为 recover.sh 的强制前置门；存在失败则删除该标记，使 recover.sh 保持阻断）。
# 覆盖场景：正常路径／空 status 有效性／M 状态文件内容变化（保全双读）／exec 不可用／仅 export 通道／
#   状态读取失败／fsck 失败／sec-test 缺失／关键文件缺失／基线不符／挂载源无法确认／挂载源形式不同经
#   金丝雀确认／重启后 ID 变化／人工 pause 流程／重启后结构漂移／pause 失败／M 状态文件内容漂移
#   （恢复核验内容回归，status 行不变仍检出）／未验证拒跑／verify 参数误用。
set -uo pipefail
command -v git >/dev/null 2>&1 || { echo "需要 git" >&2; exit 2; }
ER="${1:-}"
if [ -z "$ER" ] || [ ! -d "$ER" ]; then echo "用法: validate_recovery.sh <env-recovery 目录>" >&2; exit 2; fi
ER=$(cd "$ER" && pwd)
for f in recover.sh verify.sh; do [ -f "$ER/$f" ] || { echo "缺少 $ER/$f" >&2; exit 2; }; done

W=$(mktemp -d "${TMPDIR:-/tmp}/recovery-validate-XXXXXX")
SIM="$W/sim"; mkdir -p "$SIM"
SUM="$W/summary.txt"; : > "$SUM"
PASS=0; FAILN=0; SCEN=0
R_RC=0; V_RC=0; R_B=""; R_LOG=""
ok(){ PASS=$((PASS+1)); printf 'PASS %s\n' "$*" >> "$SUM"; }
bad(){ FAILN=$((FAILN+1)); printf 'FAIL %s\n' "$*" >> "$SUM"; }
asrt_eq(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1（期望[$3]实得[$2]）"; fi; }
chk_rc(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1（期望退出码$3 实得$2）"; fi; }
asrt_has(){ if grep -qF -- "$2" "$1" 2>/dev/null; then ok "$3"; else bad "$3（未找到: $2）"; fi; }
asrt_no(){ if grep -qF -- "$2" "$1" 2>/dev/null; then bad "$3（不应出现: $2）"; else ok "$3"; fi; }
trap '[ "${RECOVERY_VALIDATE_KEEP:-0}" = 1 ] || rm -rf "$W"' EXIT
if [ "${RECOVERY_VALIDATE_KEEP:-0}" = 1 ]; then printf 'KEEP（现场保留）: %s\n' "$W" | tee -a "$SUM"; fi

build_sim(){ # $1=场景名 $2=模式(normal|clean)
  local name="$1" mode="${2:-normal}"
  local S="$SIM/$name"
  rm -rf "$S"; mkdir -p "$S/src/env-recovery-20261103" "$S/ctrfs/tmp" "$S/state" "$S/bin" "$S/backups"
  printf '#!/usr/bin/env bash\n' > "$S/src/env-recovery-20261103/recover.sh"   # 关键项断言的标记文件
  printf '#!/usr/bin/env bash\n' > "$S/src/env-recovery-20261103/verify.sh"
  git -C "$S/src" init -q
  git -C "$S/src" config user.name t; git -C "$S/src" config user.email t@t
  mkdir -p "$S/src/src/app/src" "$S/src/docs/specs"
  printf 'code\n' > "$S/src/src/app/src/proc_session.rs"
  if [ "$mode" = normal ]; then
    mkdir -p "$S/src/src/app/tests/fixtures"
    printf 'fixture-cert\n' > "$S/src/src/app/tests/fixtures/test_cert.pem"
    printf 'fixture-two\n' > "$S/src/src/app/tests/fixtures/second.key"
    printf '# spec\n' > "$S/src/docs/specs/038-github-actions-ci.md"
    git -C "$S/src" add -A; git -C "$S/src" commit -qm baseline
    rm "$S/src/src/app/tests/fixtures/second.key"   # 未暂存删除→夹具删除状态登记
  else
    printf '# spec\n\n迭代 37 段\n' > "$S/src/docs/specs/038-github-actions-ci.md"
    mkdir -p "$S/src/docs/security/reports"
    printf 'report\n' > "$S/src/docs/security/reports/2026-11-02-038-f1-isolation-security.md"
    git -C "$S/src" add -A; git -C "$S/src" commit -qm baseline
  fi
  printf '%s' "$(git -C "$S/src" rev-parse HEAD)" > "$S/state/baseline"
  git -C "$S/src" worktree add --detach "$S/ctrfs/tmp/wt-038-f1" HEAD >/dev/null 2>&1
  if [ "$mode" = normal ]; then
    printf '\n迭代 37 段\n' >> "$S/ctrfs/tmp/wt-038-f1/docs/specs/038-github-actions-ci.md"
    mkdir -p "$S/ctrfs/tmp/wt-038-f1/docs/security/reports"
    printf 'report\n' > "$S/ctrfs/tmp/wt-038-f1/docs/security/reports/2026-11-02-038-f1-isolation-security.md"
    printf 'env-secret\n' > "$S/ctrfs/tmp/wt-038-f1/.env"
    printf 'DEEPSEEK=mock\n' > "$S/src/.env"
    printf 'local-note\n' >> "$S/src/src/app/src/proc_session.rs"   # 主工作树 M 状态文件（内容回归/漂移场景用）
    mkdir -p "$S/src/target"; printf 'junk\n' > "$S/src/target/junk.bin"
    printf 'x\n' > "$S/src/untracked-note.md"
  fi
  mkdir -p "$S/ctrfs/tmp/sec-test-038"
  printf 'case-x\n' > "$S/ctrfs/tmp/sec-test-038/run.sh"
  sed "s#__SIM__#$S#" > "$S/bin/docker" <<'SHIMEOF'
#!/usr/bin/env bash
# 模拟 docker（仅 validate_recovery.sh 临时环境）
S="__SIM__"
CID=b944a159ec468720c68dc9daf0824160c8ce6880d4498f932bafe81068e55d89
rd(){ cat "$S/state/$1" 2>/dev/null; }
wr(){ printf '%s' "$2" > "$S/state/$1"; }
st=$(rd status); [ -n "$st" ] || st=running
mapargs(){ MAPPED=(); local a
  for a in "$@"; do
    case "$a" in
      /tmp) a="$S/ctrfs/tmp";;
      /tmp/*) a="$S/ctrfs/tmp${a#/tmp}";;
      /workspace|/workspace/*) a="$S/src${a#/workspace}";;
    esac
    MAPPED+=("$a")
  done; }
cmd="$1"; shift
case "$cmd" in
  inspect)
    [ "${1:-}" = "-f" ] && shift
    fmt="$1"
    case "$fmt" in
      'ID={{.Id}} Name={{.Name}} Status={{.State.Status}} PidsLimit={{.HostConfig.PidsLimit}} Restart={{.HostConfig.RestartPolicy.Name}}')
        echo "ID=$CID Name=/agentlab-demo-dev Status=$st PidsLimit=<nil> Restart=no";;
      '{{.Name}}') echo "/agentlab-demo-dev";;
      '{{.State.Status}}') echo "$st";;
      '{{range .Mounts}}{{.Type}}  {{.Source}}  ->  {{.Destination}}{{"\n"}}{{end}}')
        src=$(rd bindsrc); [ -n "$src" ] || src="$S/src"
        printf 'bind  %s  ->  /workspace\n' "$src";;
      '{{range .Mounts}}{{if eq .Destination "/workspace"}}{{.Source}}{{end}}{{end}}')
        src=$(rd bindsrc); [ -n "$src" ] || src="$S/src"; echo "$src";;
      '{{.Id}} {{.State.Status}}')
        id="$CID"; [ -f "$S/state/id_changed" ] && id=deadbeef00
        echo "$id $st";;
      'RestartPolicy={{.HostConfig.RestartPolicy.Name}}') echo "RestartPolicy=no";;
      'ID={{.Id}} Name={{.Name}} Status={{.State.Status}} StartedAt={{.State.StartedAt}} RestartCount={{.RestartCount}}')
        echo "ID=$CID Name=/agentlab-demo-dev Status=$st StartedAt=2026-11-03T00:00:00Z RestartCount=0";;
      *) echo "shim: 未知 inspect 模板: $fmt" >&2; exit 1;;
    esac;;
  stats) rd pids || echo 4242;;
  exec)
    if [ "$(rd exec_fail)" = 1 ] || [ "$st" = paused ]; then echo "OCI runtime exec failed（模拟）" >&2; exit 126; fi
    [ "${1:-}" = "$CID" ] && shift
    case "${1:-}" in
      true) exit 0;;
      git) if [ "$(rd git_fail)" = 1 ]; then echo "git: 模拟读取失败" >&2; exit 42; fi
           shift; mapargs "$@"
           unset MSYS_NO_PATHCONV   # git.exe 是原生程序，依赖 MSYS 路径转换解析 -C /tmp/…
           exec git "${MAPPED[@]}";;
      tar) shift; mapargs "$@"
           # 写入者模拟：第二次 wt 两步法捕获（exec tar）前，对已处 M 状态的 spec 追加内容
           wtcap=0
           for ma in "${MAPPED[@]}"; do case "$ma" in wt-038-f1|*recovery-capture.tgz*) wtcap=$((wtcap+1));; esac; done
           if [ "$wtcap" -ge 2 ] && [ "$(rd mutate_second)" = 1 ]; then
             n=$(rd cpcount); [ -n "$n" ] || n=0; n=$((n+1)); wr cpcount "$n"
             if [ "$n" = 2 ]; then printf 'writer-appended-line\n' >> "$S/ctrfs/tmp/wt-038-f1/docs/specs/038-github-actions-ci.md"; fi
           fi
           exec tar "${MAPPED[@]}";;
      rm) shift; mapargs "$@"; exec rm "${MAPPED[@]}";;
      test) shift; mapargs "$@"; exec test "${MAPPED[@]}";;
      sh) case "${2:-}" in -c|-lc) ;; *) echo "shim: 未知 exec" >&2; exit 127;; esac
          m=$(printf '%s' "$3" | sed -e "s#/workspace#$S/src#g" -e "s#/tmp/#$S/ctrfs/tmp/#g" -e "s# /tmp # $S/ctrfs/tmp #g")
          exec sh -c "$m";;
      ps) echo "    1 root      0:00 /entrypoint（模拟）"; exit 0;;
      *) echo "shim: 未知 exec 命令: $*" >&2; exit 127;;
    esac;;
  cp)
    srcpath="${1:-}"; dst="${2:-}"
    if [ "$(rd cp_fail)" = 1 ]; then echo "simulated cp failure" >&2; exit 1; fi
    if [ "$dst" != "-" ]; then   # 文件形式：docker cp CID:src <本地目标>
      p="${srcpath#*:}"
      case "$p" in
        /workspace/*) t="$S/src${p#/workspace}";;
        /tmp/*) t="$S/ctrfs/tmp${p#/tmp}";;
        *) echo "shim: 未知 cp 路径: $p" >&2; exit 1;;
      esac
      [ -e "$t" ] || { echo "shim: no such path: $p" >&2; exit 1; }
      cp "$t" "$dst"; exit 0
    fi
    p="${srcpath#*:}"
    if [ "$p" = "/tmp" ]; then exec tar -C "$S/ctrfs" -cf - tmp; fi
    case "$p" in
      /workspace/*) tgt="$S/src${p#/workspace}";;
      /tmp/*) tgt="$S/ctrfs/tmp${p#/tmp}";;
      *) echo "shim: 未知 cp 路径: $p" >&2; exit 1;;
    esac
    if [ "$(rd mutate_second)" = 1 ] && [ "$p" = "/tmp/wt-038-f1" ]; then
      n=$(rd cpcount); [ -n "$n" ] || n=0; n=$((n+1)); wr cpcount "$n"
      if [ "$n" = 2 ]; then printf 'writer-appended-line\n' >> "$S/ctrfs/tmp/wt-038-f1/docs/specs/038-github-actions-ci.md"; fi
    fi
    [ -e "$tgt" ] || { echo "shim: no such path: $p" >&2; exit 1; }
    exec tar -C "$(dirname "$tgt")" -cf - "$(basename "$tgt")";;
  export)
    if [ "$(rd export_fail)" = 1 ]; then echo "simulated export failure" >&2; exit 1; fi
    exec tar -C "$S/ctrfs" -cf - tmp;;
  restart)
    wr restarted 1; wr status running
    [ "$(rd id_change)" = 1 ] && : > "$S/state/id_changed"
    echo "$CID";;
  pause) if [ "$(rd pause_fail)" = 1 ]; then echo "simulated pause failure" >&2; exit 1; fi
         wr status paused; echo "$CID";;
  unpause) wr status running; echo "$CID";;
  *) echo "shim: 未知子命令: $cmd" >&2; exit 64;;
esac
SHIMEOF
  chmod +x "$S/bin/docker"
}

env_for(){ local S="$1"
  export RECOVERY_SRC="$S/src"
  export RECOVERY_BASELINE="$(cat "$S/state/baseline")"
  export RECOVERY_MIN_WS_FILES=3   # tree_hash 剪除 .git 后 sim 工作文件仅 5-6 个
  export RECOVERY_MIN_WT_FILES=4
  export RECOVERY_BACKUP_ROOT="$S/backups"
  export RECOVERY_ARCHIVE_DEST="$S/archive"   # 证据归档重定向：模拟验证绝不写真实 /workspace
  export PATH="$S/bin:$PATH"
}
run_recover(){ local S="$SIM/$1"
  env_for "$S"
  bash "$ER/recover.sh" >"$S/recover.out" 2>"$S/recover.err"; R_RC=$?
  R_B=$(ls -d "$S"/backups/agentlab-recovery-* 2>/dev/null | tail -1)
  R_LOG="$R_B/manifest.txt"
}
run_verify(){ local S="$SIM/$1"
  env_for "$S"
  bash "$ER/verify.sh" "$R_B" >"$S/verify.out" 2>"$S/verify.err"; V_RC=$?
}

# 预置临时门标记（场景运行需要通过强制门；最终结果标记在收尾时按实况重写）
printf 'VALIDATION PASS (provisional, harness in progress)\n' > "$ER/validate-result.txt"

# ---------- S01 未验证拒跑 ----------
SCEN=$((SCEN+1)); build_sim s01-gate normal
mv "$ER/validate-result.txt" "$W/marker.bak"
env_for "$SIM/s01-gate"
bash "$ER/recover.sh" >"$SIM/s01-gate/r.out" 2>"$SIM/s01-gate/r.err"; rc=$?
chk_rc "S01 未验证拒跑（退出码≠0）" "$rc" 1
asrt_has "$SIM/s01-gate/r.err" "模拟验证" "S01 提示先运行模拟验证"
mv "$W/marker.bak" "$ER/validate-result.txt"

# ---------- S02 正常路径 ----------
SCEN=$((SCEN+1)); build_sim s02-normal normal
run_recover s02-normal
chk_rc "S02 正常路径恢复成功" "$R_RC" 0
asrt_has "$R_LOG" "【里程碑：备份完成】" "S02 备份完成里程碑"
asrt_has "$R_LOG" "【里程碑：重启完成】" "S02 重启完成里程碑（ID 未变且 running）"
asrt_no "$R_LOG" "阻断（不重启）" "S02 全程无阻断"
[ -f "$SIM/s02-normal/state/restarted" ] && ok "S02 restart 恰好执行一次" || bad "S02 restart 未执行"
asrt_no "$R_B/workspace-full.list" ".env" "S02 workspace 排除 .env"
asrt_no "$R_B/workspace-full.list" "junk.bin" "S02 workspace 排除 target/"
asrt_no "$R_B/workspace-full.list" "test_cert.pem" "S02 workspace 排除已跟踪夹具（不回填）"
asrt_has "$R_B/fixtures-current.list" "test_cert.pem" "S02 夹具当前内容独立保全"
asrt_has "$R_B/fixtures-status.txt" "deleted src/app/tests/fixtures/second.key" "S02 夹具删除状态登记"
asrt_has "$R_B/status-ws-copy.txt" " D src/app/tests/fixtures/test_cert.pem" "S02 副本如实显示排除（未宣称回填后＝原目录备份）"
asrt_no "$R_B/wt-038-f1.list" ".env" "S02 wt 备份排除敏感文件"
if [ -f "$R_B/ws-double-read-diff.txt" ] && [ -f "$R_B/wt-double-read-diff.txt" ] && [ -f "$R_B/sec-double-read-diff.txt" ]; then
  ok "S02 三组内容双读证据落盘"; else bad "S02 双读证据缺失"; fi
run_verify s02-normal
chk_rc "S02 核验通过（退出码 0）" "$V_RC" 0
asrt_has "$R_B/post-restart-verify.txt" "【里程碑：核验完成】" "S02 核验完成里程碑"
asrt_has "$R_LOG" "写入控制模式＝协调停止" "S02 默认不用 pause（对照实证其使通道失效），协调停止＋双读在案"
asrt_has "$R_B/post-restart-verify.txt" "workspace live 与备份逐哈希一致" "S02 内容回归 PASS（不只看 status 行）"
bash "$ER/verify.sh" >/dev/null 2>&1; rc=$?
chk_rc "S02 verify 缺参数拒绝（显式目录要求）" "$rc" 2
bash "$ER/verify.sh" /nonexistent-xyz >/dev/null 2>&1; rc=$?
chk_rc "S02 verify 无效目录拒绝" "$rc" 2

# ---------- S03 空 status 有效 ----------
SCEN=$((SCEN+1)); build_sim s03-clean clean
run_recover s03-clean
chk_rc "S03 干净树恢复成功" "$R_RC" 0
asrt_eq "S03 备份副本 status 为空（干净树）" "$(grep -c . "$R_B/status-ws-copy.txt" 2>/dev/null)" 0
run_verify s03-clean
chk_rc "S03 核验通过" "$V_RC" 0
asrt_has "$R_B/post-restart-verify.txt" "（0 条）" "S03 成功执行且输出为空的 git status 是有效结果"

# ---------- S04 写入者内容变化检出 ----------
SCEN=$((SCEN+1)); build_sim s04-writer normal
printf '1' > "$SIM/s04-writer/state/mutate_second"
run_recover s04-writer
chk_rc "S04 写入者被内容双读检出（非零退出）" "$R_RC" 1
asrt_has "$R_LOG" "wt 双读内容差异" "S04 双读检出 M 状态文件的内容变化（status 行不变仍检出）"
asrt_no "$R_LOG" "【里程碑：备份完成】" "S04 未登记备份完成"
[ ! -f "$SIM/s04-writer/state/restarted" ] && ok "S04 未重启" || bad "S04 不应重启却重启了"

# ---------- S05 exec 全程不可用（cp 通道） ----------
SCEN=$((SCEN+1)); build_sim s05-execdown normal
printf '1' > "$SIM/s05-execdown/state/exec_fail"
run_recover s05-execdown
chk_rc "S05 exec 不可用时经 cp 通道完成保全+重启" "$R_RC" 0
asrt_has "$R_LOG" "wt 首次捕获通道=docker-cp" "S05 cp 通道（不在容器内建进程）"
asrt_has "$R_LOG" "【里程碑：重启完成】" "S05 重启完成"
run_verify s05-execdown
chk_rc "S05 重启后 exec 仍不可用→核验不得判完成" "$V_RC" 1
asrt_no "$R_B/post-restart-verify.txt" "【里程碑：核验完成】" "S05 未误报核验完成"

# ---------- S06 仅 export 通道（简化命名目录提取） ----------
SCEN=$((SCEN+1)); build_sim s06-export normal
printf '1' > "$SIM/s06-export/state/exec_fail"
printf '1' > "$SIM/s06-export/state/cp_fail"
run_recover s06-export
chk_rc "S06 cp/exec 均不可用时经 export 完成保全" "$R_RC" 0
asrt_has "$R_LOG" "docker-export" "S06 export 通道启用"
asrt_has "$R_B/wt-038-f1.list" "wt-038-f1/src/app/src/proc_session.rs" "S06 命名目录递归提取含深层文件"
run_verify s06-export
chk_rc "S06 重启后 exec 不可用→核验不判完成" "$V_RC" 1

# ---------- S07 状态读取失败 ----------
SCEN=$((SCEN+1)); build_sim s07-statusfail normal
printf '1' > "$SIM/s07-statusfail/state/git_fail"
run_recover s07-statusfail
chk_rc "S07 状态读取失败不阻断保全（内容双读承载，缺口登记）" "$R_RC" 0
asrt_has "$R_LOG" "缺口" "S07 状态缺口如实登记"
run_verify s07-statusfail
chk_rc "S07 核验端状态读取失败→不得判完成" "$V_RC" 1
asrt_has "$R_B/post-restart-verify.txt" "必要检查缺失" "S07 必要检查缺失如实报告"

# ---------- S08 fsck 失败 ----------
SCEN=$((SCEN+1)); build_sim s08-fsck normal
tree=$(git -C "$SIM/s08-fsck/src" rev-parse 'HEAD^{tree}')
obj="$SIM/s08-fsck/src/.git/objects/${tree:0:2}/${tree:2}"
chmod u+w "$obj" 2>/dev/null   # git 落盘对象为只读，不 chmod 则 dd 静默失败、损坏不会生效
printf 'X' | dd of="$obj" bs=1 seek=2 conv=notrunc 2>/dev/null
run_recover s08-fsck
chk_rc "S08 fsck 失败被阻断（非零退出）" "$R_RC" 1
asrt_has "$R_LOG" "fsck 失败" "S08 fsck 阻断信息"
asrt_no "$R_LOG" "【里程碑：备份完成】" "S08 未登记备份完成"
[ ! -f "$SIM/s08-fsck/state/restarted" ] && ok "S08 未重启" || bad "S08 不应重启却重启了"

# ---------- S09 sec-test 缺失 ----------
SCEN=$((SCEN+1)); build_sim s09-nosec normal
rm -rf "$SIM/s09-nosec/ctrfs/tmp/sec-test-038"
run_recover s09-nosec
chk_rc "S09 必要取证文件缺失被阻断" "$R_RC" 1
[ -f "$R_B/tmp-others-list.txt" ] && ok "S09 /tmp 枚举清单落盘" || bad "S09 缺 /tmp 枚举清单"
asrt_no "$R_LOG" "【里程碑：重启完成】" "S09 未重启"

# ---------- S10 wt 关键文件缺失 ----------
SCEN=$((SCEN+1)); build_sim s10-nokey normal
rm -f "$SIM/s10-nokey/ctrfs/tmp/wt-038-f1/src/app/src/proc_session.rs"
run_recover s10-nokey
chk_rc "S10 关键文件缺失被阻断" "$R_RC" 1
asrt_has "$R_LOG" "缺关键项" "S10 关键项断言触发"
[ ! -f "$SIM/s10-nokey/state/restarted" ] && ok "S10 未重启" || bad "S10 不应重启却重启了"

# ---------- S11 基线不符 ----------
SCEN=$((SCEN+1)); build_sim s11-basewrong normal
env_for "$SIM/s11-basewrong"
export RECOVERY_BASELINE=0000000000000000000000000000000000000000
bash "$ER/recover.sh" >"$SIM/s11-basewrong/r.out" 2>"$SIM/s11-basewrong/r.err"; rc=$?
chk_rc "S11 HEAD 与基线不符被阻断" "$rc" 1
asrt_has "$SIM/s11-basewrong/r.out" "HEAD 与预期不符" "S11 阻断原因可读（die 走 stdout）"
[ ! -f "$SIM/s11-basewrong/state/restarted" ] && ok "S11 未重启" || bad "S11 不应重启却重启了"

# ---------- S12 挂载源无法确认（严格确认，无尾部相似放行） ----------
SCEN=$((SCEN+1)); build_sim s12-srcmismatch normal
printf '/mnt/x/wrong' > "$SIM/s12-srcmismatch/state/bindsrc"
printf '1' > "$SIM/s12-srcmismatch/state/cp_fail"
env_for "$SIM/s12-srcmismatch"
bash "$ER/recover.sh" >"$SIM/s12-srcmismatch/r.out" 2>"$SIM/s12-srcmismatch/r.err"; rc=$?
chk_rc "S12 挂载源无法确认被阻断" "$rc" 1
asrt_has "$SIM/s12-srcmismatch/r.out" "无法确认实际挂载源" "S12 阻断原因（金丝雀与路径核对均未通过）"
[ ! -f "$SIM/s12-srcmismatch/state/restarted" ] && ok "S12 未重启" || bad "S12 不应重启却重启了"

# ---------- S13 挂载源形式不同但经金丝雀证实对应 ----------
SCEN=$((SCEN+1)); build_sim s13-srcform normal
printf 'D:\\AgentLab\\projects\\demo' > "$SIM/s13-srcform/state/bindsrc"
run_recover s13-srcform
chk_rc "S13 源路径形式不同、金丝雀证实对应→放行" "$R_RC" 0
asrt_has "$R_LOG" "挂载源确认方式=canary" "S13 金丝雀确认方式登记"

# ---------- S14 重启后 ID 变化 → 不登记重启完成 ----------
SCEN=$((SCEN+1)); build_sim s14-idchange normal
printf '1' > "$SIM/s14-idchange/state/id_change"
run_recover s14-idchange
chk_rc "S14 重启后 ID 变化被判异常（非零退出）" "$R_RC" 1
[ -f "$SIM/s14-idchange/state/restarted" ] && ok "S14 restart 已执行（如实记录）" || bad "S14 restart 应已执行"
asrt_no "$R_LOG" "【里程碑：重启完成】" "S14 不登记重启完成"

# ---------- S15 人工 pause 流程 ----------
SCEN=$((SCEN+1)); build_sim s15-paused normal
printf 'paused' > "$SIM/s15-paused/state/status"
run_recover s15-paused
chk_rc "S15 人工冻结期间保全＋unpause 后重启" "$R_RC" 0
asrt_has "$R_LOG" "解除冻结已证实: Status=running" "S15 unpause 至重启的处理过程在案"
asrt_has "$R_LOG" "【里程碑：重启完成】" "S15 重启完成"

# ---------- S16 重启后漂移被检出 ----------
SCEN=$((SCEN+1)); build_sim s16-drift normal
run_recover s16-drift
chk_rc "S16 预置恢复成功" "$R_RC" 0
printf 'new\n' > "$SIM/s16-drift/ctrfs/tmp/wt-038-f1/drift-file.md"
run_verify s16-drift
chk_rc "S16 重启后漂移被检出（非零退出）" "$V_RC" 1
asrt_has "$R_B/post-restart-verify.txt" "wt status 与保全前不一致" "S16 漂移差异报告"
asrt_no "$R_B/post-restart-verify.txt" "【里程碑：核验完成】" "S16 不误报核验完成"

# ---------- S17 pause 不可用 → 降级继续（RECOVERY_PAUSE=1 显式启用 pause 以覆盖此路径） ----------
SCEN=$((SCEN+1)); build_sim s17-pausefail normal
printf '1' > "$SIM/s17-pausefail/state/pause_fail"
RECOVERY_PAUSE=1 run_recover s17-pausefail
chk_rc "S17 pause 不可用时降级继续（不阻断）" "$R_RC" 0
asrt_has "$R_LOG" "docker pause 不可用" "S17 降级声明在案"
asrt_has "$R_LOG" "三组双读逐字节一致（辅助证据，pause 不可用）" "S17 保全条件证据按降级口径登记"
asrt_has "$R_LOG" "【里程碑：备份完成】" "S17 备份完成"
asrt_has "$R_LOG" "【里程碑：重启完成】" "S17 重启完成"

# ---------- S18 M 状态文件内容漂移（status 行不变，内容回归必检） ----------
SCEN=$((SCEN+1)); build_sim s18-mdrift normal
run_recover s18-mdrift
chk_rc "S18 预置恢复成功" "$R_RC" 0
printf 'drift-M-wt\n' >> "$SIM/s18-mdrift/ctrfs/tmp/wt-038-f1/docs/specs/038-github-actions-ci.md"
printf 'drift-M-ws\n' >> "$SIM/s18-mdrift/src/src/app/src/proc_session.rs"
run_verify s18-mdrift
chk_rc "S18 M 状态文件内容漂移被内容回归检出（非零退出）" "$V_RC" 1
asrt_has "$R_B/post-restart-verify.txt" "PASS: wt status 与保全前一致" "S18 status 行比较通过（证明漂移不在 status 层）"
asrt_has "$R_B/post-restart-verify.txt" "workspace live 与备份内容差异" "S18 workspace M 文件内容差异检出"
asrt_has "$R_B/post-restart-verify.txt" "wt live 与备份内容差异" "S18 wt M 文件内容差异检出"
asrt_no "$R_B/post-restart-verify.txt" "【里程碑：核验完成】" "S18 不误报核验完成"

# ---------- 收尾 ----------
cp "$SUM" "$ER/validate-summary.txt"
printf '\n===== 汇总: 场景=%s 通过=%s 失败=%s =====\n' "$SCEN" "$PASS" "$FAILN" >> "$ER/validate-summary.txt"
if [ "$FAILN" -eq 0 ]; then
  printf 'VALIDATION PASS %s scenarios=%s checks=%s\n' "$(date '+%F %T')" "$SCEN" "$PASS" > "$ER/validate-result.txt"
  printf '全部通过。已写 %s 与 %s\n' "$ER/validate-result.txt" "$ER/validate-summary.txt"
  exit 0
fi
rm -f "$ER/validate-result.txt"
printf '存在失败（%s 项）——已删除 validate-result.txt，recover.sh 保持阻断；证据见 %s\n' "$FAILN" "$ER/validate-summary.txt"
exit 1
