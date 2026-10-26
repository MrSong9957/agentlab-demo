#!/usr/bin/env bash
# 033 真实沙箱验收（宿主机执行；Docker Desktop / WSL2 backend 语义）。
#
# 用法：
#   WT_HOST_PATH='<worktree 宿主路径>' [ENGINE=docker] [KEEP_IMAGE=0] \
#     bash 033-sandbox-accept.sh
#   WT_HOST_PATH 示例（WSL 视图）：/mnt/d/AgentLab/.worktrees/033-tool-sandbox
#
# 路径语义（033 收口轮）：--security-opt seccomp 使用 **CLI 本地可读的
# HOST_JSON**（worktree 内生成物，与引擎同侧文件系统）；worktree 的 bind
# mount 沿用 WT_HOST_PATH，两者分别处理、互不转换。
#
# 流程：默认策略对照 → 精确 seccomp → probe_available 断言 → 真实分支套件
# → PID 视图 → 全量（冻结代码）→ 清理（仅本轮容器/镜像）。
# 退出码：0＝全部通过；非 0＝首个失败点。日志附时间戳逐条打印。
set -uo pipefail

WT_HOST_PATH="${WT_HOST_PATH:?必须设置 worktree 的宿主路径}"
ENGINE="${ENGINE:-docker}"
ACCEPT_IMG="agentlab-033-accept:latest"
DEFAULT_CT="tchat-033-ctl-default"
ACCEPT_CT="tchat-033-accept"
WORKDIR_IN="/wt033"
TARGET_DIR_IN="/tmp/wt033-target"
CARGO_HOME_IN="/tmp/wt033-cargo"
KEEP_IMAGE="${KEEP_IMAGE:-0}"
LOG_DIR="${LOG_DIR:-$WT_HOST_PATH/docs/logs/033-tool-sandbox}"

summary=()
note()  { echo "[$(date +%H:%M:%S)] $*"; }
pass()  { summary+=("PASS: $*"); note "PASS  $*"; }
fail()  { summary+=("FAIL: $*"); note "FAIL  $*"; }
die()   { fail "$*"; printf '%s\n' "${summary[@]}"; exit 1; }

# 7b 夹具创建标记：仅清理本轮确认创建的工作区临时文件（防误删既有文件）。
CREATED_ENV=0

cleanup() {
  "$ENGINE" rm -f "$DEFAULT_CT" "$ACCEPT_CT" >/dev/null 2>&1 || true
  [ "$KEEP_IMAGE" = "1" ] || "$ENGINE" rmi -f "$ACCEPT_IMG" >/dev/null 2>&1 || true
  if [ "$CREATED_ENV" = "1" ] && [ -n "${WT_HOST_PATH:-}" ]; then
    rm -f "$WT_HOST_PATH/.env" "$WT_HOST_PATH/acc-write.txt" 2>/dev/null || true
  fi
}
trap cleanup EXIT

# ── 1. 引擎版本与默认 seccomp 来源固定 ─────────────────────────────
note "1/8 引擎版本与默认 seccomp 来源"
ENGINE_VER=$("$ENGINE" version --format '{{.Server.Version}}') || die "无法连接 docker 引擎"
DEFAULT_JSON="/tmp/033-default-seccomp.json"
BASELINE_SRC=""
# 基线获取（两条仓库布局，先新后旧；任一成功即固定对应关系，全败＝拒绝继续）：
# · moby v28 起默认 profile 移入独立模块 github.com/moby/profiles，引擎源码
#   tag 形如 docker-v<完整版本>——按 vendor/modules.txt 锁定的模块版本取
#   default.json，与引擎实际编译的策略逐版本对应（2026-10-27 实测链条：
#   docker-v29.8.2 → moby/profiles/seccomp v0.2.3）。
# · 旧布局（引擎 ≤v27.x）：主仓库 tag v<major.minor> 的 profiles/seccomp/default.json。
# 下载通道：api.github.com 优先（Accept raw media 直接回原始文件；本网络全天
# 稳定，未认证限额 60 req/h 足够）——raw.githubusercontent 存在分钟级连接
# 重置窗口（2026-10-27 实测 7 分钟连续失败），仅作回退。
gh_fetch() { # gh_fetch <repo> <path> <ref> <outfile>
  curl -fsSL --retry 3 --retry-delay 3 --retry-all-errors -m 60 \
      -H "Accept: application/vnd.github.raw" \
      "https://api.github.com/repos/$1/contents/$2?ref=$3" -o "$4" 2>/dev/null \
    && return 0
  curl -fsSL --retry 3 --retry-delay 3 --retry-all-errors -m 60 \
      "https://raw.githubusercontent.com/$1/$3/$2" -o "$4" 2>/dev/null
}

fetch_baseline() {
  if gh_fetch moby/moby vendor/modules.txt "docker-v${ENGINE_VER}" /tmp/033-modules.txt; then
    PROFILES_VER=$(grep -m1 '^# github.com/moby/profiles/seccomp ' /tmp/033-modules.txt | awk '{print $NF}')
    if [ -n "${PROFILES_VER:-}" ] \
       && gh_fetch moby/profiles seccomp/default.json "seccomp/${PROFILES_VER}" "$DEFAULT_JSON"; then
      BASELINE_SRC="moby/profiles/seccomp ${PROFILES_VER}（引擎 docker-v${ENGINE_VER} vendor 锁定）"
      return 0
    fi
  fi
  if gh_fetch moby/moby profiles/seccomp/default.json "v${ENGINE_VER%.*}" "$DEFAULT_JSON"; then
    BASELINE_SRC="moby 主仓库 v${ENGINE_VER%.*} profiles/seccomp/default.json（旧布局）"
    return 0
  fi
  return 1
}
BASELINE_TRY=0
until fetch_baseline; do
  BASELINE_TRY=$((BASELINE_TRY + 1))
  if [ "$BASELINE_TRY" -ge 2 ]; then break; fi
  note "基线下载两通道均未成（第 ${BASELINE_TRY} 轮），20s 后重试"
  sleep 20
done
if [ -n "$BASELINE_SRC" ]; then
  note "引擎 $ENGINE_VER → 基线：$BASELINE_SRC"
else
  # 033 收口修正：基线无法取得/无法确认对应 → 非零退出，不回退旧版本继续、
  # 不启动验收容器（避免在未经确认的策略基线上做真实验收）。
  note "【宿主执行前检查】引擎 ${ENGINE_VER} 对应的默认 seccomp 基线（两种布局）均无法取得，" \
       "无法确认对应关系——拒绝继续"
  printf '%s\n' "${summary[@]}" 2>/dev/null || true
  echo "请按引擎实际版本提供/核对基线后重试。"
  exit 1
fi
python3 "$(dirname "$0")/033-seccomp-patch.py" "$DEFAULT_JSON" /tmp/033-patched-seccomp.json \
  || die "seccomp 补丁生成失败"

# seccomp 路径＝**CLI 本地可读文件**（与 worktree bind mount 的 WT_HOST_PATH
# 分别处理：bind 挂载点由容器看，seccomp 由引擎侧按本地路径读）。
HOST_JSON="$WT_HOST_PATH/scripts/acceptance/generated-033-seccomp.json"
cp /tmp/033-patched-seccomp.json "$HOST_JSON" || die "json 写入 worktree 失败"
# 启动前回归检查：文件存在且可读，否则不启动验收容器。
[ -f "$HOST_JSON" ] && [ -r "$HOST_JSON" ] || die "seccomp 参数目标不可读：$HOST_JSON"
python3 -c "import json,sys; json.load(open(sys.argv[1]))" "$HOST_JSON" \
  || die "seccomp 文件不是合法 JSON：$HOST_JSON"
pass "seccomp 参数目标就绪（CLI 本地可读）：$HOST_JSON"
# 边界声明：seccomp 放行 ≠ 授权——mount/pivot_root/clone 放行后，内核调用点
# 仍执行 ns 范围能力检查（无能力即 EPERM）；seccomp 规则仅决定「syscall 是否
# 到达内核」，不改变内核的权限判定。

# ── 2. 构建验收镜像（同镜像 + bwrap；上下文仅本目录，无源码/凭据）──
note "2/8 构建验收镜像 $ACCEPT_IMG"
"$ENGINE" build -q -f "$(dirname "$0")/033-accept-bwrap.Dockerfile" \
  -t "$ACCEPT_IMG" "$(dirname "$0")" >/dev/null || die "验收镜像构建失败"
pass "验收镜像构建（agentlab-demo-dev + bubblewrap）"

# ── 3. 默认策略对照（同一镜像同一用户；若默认已可用则取消补丁）─────
note "3/8 默认策略对照（unshare --user）"
CTL_OUT=$("$ENGINE" run --rm --user dev "$ACCEPT_IMG" \
  sh -c 'unshare --user --map-root-user true 2>&1; echo "rc=$?"')
echo "$CTL_OUT" | tail -1
if echo "$CTL_OUT" | grep -q "rc=0"; then
  note "默认策略已可用 userns：取消 seccomp 修改，后续以默认策略继续"
  SECCOMP_ARGS=()
  pass "对照：默认策略可用（seccomp 补丁取消）"
else
  SECCOMP_ARGS=(--security-opt "seccomp=$HOST_JSON")
  pass "对照：默认策略拒绝 userns（复现探针结论，非孤证）"
fi

# ── 4. 起验收容器（独立；不挂 Agent 凭据卷/socket；目标目录容器内私有）─
note "4/8 启动验收容器（user=dev；worktree bind：$WT_HOST_PATH）"
"$ENGINE" run -d --name "$ACCEPT_CT" --user dev \
  ${SECCOMP_ARGS[@]+"${SECCOMP_ARGS[@]}"} \
  -v "$WT_HOST_PATH:$WORKDIR_IN" \
  -e "CARGO_TARGET_DIR=$TARGET_DIR_IN" -e "CARGO_HOME=$CARGO_HOME_IN" \
  -w "$WORKDIR_IN" "$ACCEPT_IMG" sleep infinity >/dev/null || die "验收容器启动失败"
RUN() { "$ENGINE" exec "$ACCEPT_CT" "$@"; }

# /run/WSL 预创建（2026-10-27 宿主首轮实测修正）：容器非 WSL 发行版，该目录
# 天然不存在——预建后产品（上游 mask_wsl_interop 的 is_dir 判定）与 7a/7b
# 独立探针的 `--tmpfs /run/WSL` 遮蔽按 WSL 部署形态真实执行；不预建时产品按
# 存在性正确跳过遮蔽（该分支由单测 wsl_interop_mask_follows_dir_presence_flag
# 覆盖，本验收走「存在＝遮蔽」的更强分支）。无遮蔽路径下 bwrap 须在 ro-bind
# 根上 mkdir 该目录，EROFS 直接失败——正是首轮暴露的产品缺陷（已修复）。
"$ENGINE" exec -u root "$ACCEPT_CT" mkdir -p /run/WSL >/dev/null \
  || die "预创建 /run/WSL 失败"

# ── 5. probe_available 断言（与产品 probe_bwrap_at 同参数干跑）──────
# 注：不以 util-linux unshare(1) 作断言——seccomp 补丁刻意只放行 bwrap 的两种
# clone 等值形态、不放行 unshare syscall（bwrap 0.9.0 不调用）；首轮实测中
# unshare 在补丁下仍 EPERM 属预期，断言它会误判「策略层仍有阻断」。
note "5/8 probe_available 断言（bwrap 干跑：ro-bind+userns+cap-drop）"
RUN sh -c 'bwrap --version' || die "容器内 bwrap 不可用"
RUN bwrap --ro-bind / / --unshare-user --unshare-ipc --cap-drop ALL -- /bin/true \
  || die "bwrap 干跑失败＝probe_available false——停止，不进入真实分支"
pass "probe_available == true（bwrap 干跑通过，与产品探测同参数）"

# 依赖预取（镜像内无 registry 缓存；fetch 后 --offline 全量可行）
note "5b/8 cargo fetch（隔离容器私有 CARGO_HOME，不与 dev 容器共享）"
RUN sh -c "cargo fetch --locked" >/dev/null || die "cargo fetch 失败"

# ── 6. 真实分支套件（第 5 步已断言 probe 可用；测试内 probe_available() 运行时
# 判定必然走真实分支——无静默转入 failIfUnavailable 的可能，后续 6b 独立探针
# 再给一层直接证据）────────────────────────────────────────────────
note "6/8 真实沙箱套件"
LIB_OUT=$(RUN sh -c "cargo test --locked --offline --lib sandbox:: -- --nocapture 2>&1") \
  || { echo "$LIB_OUT" | tail -30; die "sandbox 单测失败"; }
# grep 断言一律 here-string（grep -q 命中即退；cargo 全量输出可达 ~180KB，
# `echo "$OUT" | grep -q` 在 pipefail 下命中早期位置时 echo 必收 SIGPIPE(141)
# → 管线非零 → 误判失败。2026-10-27 宿主实测定位，普通回归步两次误杀）。
grep -q "16 passed" <<< "$LIB_OUT" || { echo "$LIB_OUT" | tail -20; die "lib 数量不符（期望 16 passed）"; }
pass "lib sandbox 16/16"
for t in exec_sandbox mcp_stdio_sandbox apply_patch_sandbox; do
  OUT=$(RUN sh -c "cargo test --locked --offline --test $t -- --nocapture 2>&1") \
    || { echo "$OUT" | tail -30; die "$t 失败"; }
  grep -q " 0 failed" <<< "$OUT" || { echo "$OUT" | tail -20; die "$t 有失败"; }
  pass "$t 全绿（真实分支）"
done
# 普通/其余回归（对照面）
OUT=$(RUN sh -c "cargo test --locked --offline --test exec_command --test mcp_stdio 2>&1") \
  || { echo "$OUT" | tail -30; die "普通模式回归失败"; }
grep -q " 0 failed" <<< "$OUT" \
  || { echo "$OUT" | tail -40; die "普通模式回归有失败（输出未见 0 failed）"; }
pass "普通模式回归 18＋35 全绿"

# 7a PID 视图（G3 关闭实证）：沙箱（--unshare-pid＋--as-pid-1＋--proc）内
# pid 1 应为**载荷进程自身**（--as-pid-1 实测语义：不建中间 init，命令直接
# 作为子 PID 命名空间的 pid 1；2026-10-27 宿主实测 pid1=sh，初版断言写反），
# 可见进程数个位级（容器面进程全部不可见）。
note "7a PID 视图检查"
VIEW=$(RUN sh -c 'bwrap --as-pid-1 --ro-bind / / --dev /dev --tmpfs /run/WSL \
  --unshare-user --unshare-pid --unshare-ipc --proc /proc --cap-drop ALL \
  -- sh -c "echo pid1=\$(cat /proc/1/comm); echo nproc=\$(ls /proc | grep -cE \"^[0-9]+$\")"')
echo "$VIEW"
echo "$VIEW" | grep -q "pid1=sh" || die "沙箱内 pid 1 非载荷进程（PID ns 未生效）"
NPROC=$(echo "$VIEW" | grep "nproc=" | cut -d= -f2)
[ "${NPROC:-99}" -le 8 ] || die "沙箱 /proc 进程数异常（$NPROC）——PID 隔离可疑"
pass "PID 视图：沙箱内 pid1=载荷进程，可见进程数=$NPROC（容器面进程不可见，G3 关闭实证）"

# 7b 独立假凭据探针（不经测试框架的直接证据链）：
#    ①沙箱外文件存在且含假标记 → ②沙箱内该路径为空绑定且取不到正文（见下）→ ③正常文件可读写。
note "7b 独立假凭据探针（锚定根 .env：外有内无＋正常读写）"
# rm -f 防残留：bwrap 会为 --ro-bind /dev/null 预创建缺失挂载点（0 字节只读，
# 宿主首轮实测踩坑）——上一轮探针若中断可能留下该残留，导致夹具写入 EACCES。
RUN rm -f "$WORKDIR_IN/.env"
FIXTURE=$(RUN sh -c 'printf "FAKE-SECRET-LINE=1\n" > "$0/.env" && wc -c < "$0/.env"' "$WORKDIR_IN")
CREATED_ENV=1
[ "$FIXTURE" = "19" ] || die "沙箱外夹具异常（期望 19 字节）：$FIXTURE"
# 与产品 create_bwrap_flags 同构（锚定根=$WORKDIR_IN）：ro-bind 全盘＋--dev，
# --bind 工作区重开写，--ro-bind /dev/null 遮蔽 <工作区>/.env（嵌套 carve-out
# 在 bind 之后，后遮蔽覆盖先 bind——缺这两项时「内无」与「工作区可写」必然
# 失败，033 宿主首轮实测修正）。temp 根 bind 与本探针两项断言无关，不复制。
SANDBOX_ENV="bwrap --as-pid-1 --new-session --die-with-parent \
  --ro-bind / / --dev /dev --bind $WORKDIR_IN $WORKDIR_IN \
  --ro-bind /dev/null $WORKDIR_IN/.env --tmpfs /run/WSL \
  --unshare-user --unshare-pid --unshare-ipc --proc /proc --cap-drop ALL"
# 「内无」的实测语义（2026-10-27 宿主实测修正）：userns 内非特权 bind 被内核
# 强制 nodev，open /dev/null 绑定＝EACCES 而非读到 0 字节；上游 perms 000 方案
# 同样不可 open——「取不到正文」即目标。断言与产品 A2 对齐：stat 级空＋标记缺失。
INNER_STAT=$(RUN sh -c "$SANDBOX_ENV -- stat -c '%F size=%s' $WORKDIR_IN/.env")
echo "沙箱内 .env → $INNER_STAT"
echo "$INNER_STAT" | grep -q "size=0" || die "沙箱内 .env 非 /dev/null 空绑定：$INNER_STAT"
RUN sh -c "$SANDBOX_ENV -- sh -c 'test ! -s $WORKDIR_IN/.env'" \
  || die "沙箱内锚定根 .env 非「空/不可读」"
RUN sh -c "$SANDBOX_ENV -- sh -c 'cat $WORKDIR_IN/.env' 2>/dev/null | grep -q FAKE-SECRET-LINE" \
  && die "沙箱内读到了假标记——隔离失败"
RUN sh -c "$SANDBOX_ENV -- sh -c 'echo data > $WORKDIR_IN/acc-write.txt && cat $WORKDIR_IN/acc-write.txt' | grep -q data" \
  || die "沙箱内工作区正常读写失败"
pass "假凭据探针：沙箱外 19 字节含标记 → 沙箱内空绑定取不到正文；工作区读写正常"
# 7c G4-R1/G3 佐证：Update 走 /proc/self/fd（沙箱内 /proc/self 存在性已由 7a 的
# 新 procfs 保证；apply_patch 三型在 A4 真实分支完成）。
note "7c apply_patch 三型已随 A4 真实分支通过（Update＝G4-R1 修复实证）"

# ── 8. 全量（冻结代码；用户指定命令）───────────────────────────────
note "8/8 全量（串行；预计 1–2 小时）"
# exec -t：分配 PTY——tchat-tui 的 security_untrusted_text 经 crossterm 查询终端
# 尺寸，无控制终端时失败（A/B 实验 2026-10-27：无 -t 该目标 1 failed 且与
# seccomp/bwrap 无关——纯 dev 镜像默认策略同样失败；-t 下 12/12 全绿。dev 容器
# 历史全绿亦运行于带 TTY 的交互会话）。-t 仅改变输出传输形态（ONLCR），命令
# 本体逐字不变；下方 RESULT 解析的字段不受行尾 \r 影响。
"$ENGINE" exec -t "$ACCEPT_CT" sh -c "env CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 sh -c \
  'cargo check --locked --offline && cargo test --workspace --no-fail-fast --locked --offline'" \
  > "$LOG_DIR/acceptance-full.log" 2>&1 \
  || { tail -30 "$LOG_DIR/acceptance-full.log"; die "全量失败（日志 $LOG_DIR/acceptance-full.log）"; }
RESULT=$(grep "test result:" "$LOG_DIR/acceptance-full.log" | awk -F'[ .;]+' \
  '{p+=$4; f+=$6; i+=$8} END {print p"/"f"/"i}')
# 单括号 [ 不做通配匹配（[[ ]] 才有该语义；初版 `"0/"*` 在 bash [ ] 中按字面
# 串比较恒假，2026-10-27 宿主实测 3506/0/3 被误判）——显式分段解析。
TOTAL_N=${RESULT%%/*}; FAILED_N=${RESULT#*/}; FAILED_N=${FAILED_N%%/*}
[ "$TOTAL_N" -ge 3500 ] && [ "$FAILED_N" = "0" ] \
  || die "全量数字异常：$RESULT（期望 ≥3500 passed / 0 failed）"
pass "全量 68 目标 $RESULT（0 failed）"

printf '\n═══ 验收汇总 ═══\n'
printf '%s\n' "${summary[@]}"
echo "结论：真实沙箱验收通过（全部 PASS）——可进入合并决策。"
