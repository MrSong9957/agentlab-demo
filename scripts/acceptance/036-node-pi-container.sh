#!/usr/bin/env bash
# 036/037 运行验收（容器内执行；在已构建的 agentlab-demo-dev 容器中、以 dev 运行）。
#
# 用法（宿主）：
#   docker exec agentlab-demo-dev bash /workspace/scripts/acceptance/036-node-pi-container.sh
# 或容器内直接：bash scripts/acceptance/036-node-pi-container.sh
#
# 覆盖规格场景（docs/specs/036-node-pi-container.md §5、037-cc-switch-container.md §5）：
#   1. dev 在交互/登录/非登录 shell 调用 node/npm/pi/cc-switch 版本与帮助，
#      入口全部来自镜像系统路径（036 R1–R3、037 R1–R2）。
#   2. 镜像入口优先：持久卷内不得存在同名可执行阴影（036 §2 旧启动器风险）。
#   3. 持久挂载存在且 dev 可写（036 R4–R5、037 R3）。
#   4. CC-Switch 全新隔离 HOME 初始化真实数据库并保留假标记（037 §5 场景 4）。
#   5. Pi 全新隔离 HOME 首次写入创建自身数据目录（036 §5 场景 3）。
#   6. 数据跨容器重建保留：持久文件 mtime 早于本次容器启动时间（036 §5 场景 5）。
#   每节含对照用例（缺程序／缺挂载／阴影/时钟方向），对照必须失败才算断言有效。
#
# 不在本脚本范围（需宿主 docker，见 README「主动更新」）：
#   临时容器/临时卷的停启与删除重建、docker pull + --no-cache 刷新构建。
#   固定名容器 agentlab-demo-dev 的删除重建由部署者在宿主执行，本脚本只读取其结果。
#
# 全程不读写真实凭证：CLI 调用一律 env -i + 临时 HOME；真实数据库仅做只读副本检查。
# 退出码：0＝全部通过；1＝存在失败；对照用例失败按预期计 PASS。
set -uo pipefail

PASS=0; FAIL=0
note() { echo "[$(date +%H:%M:%S)] $*"; }
pass() { PASS=$((PASS+1)); note "PASS  $*"; }
fail() { FAIL=$((FAIL+1)); note "FAIL  $*"; }
# 对照用例：断言"应当失败"；失败才证明检测有效。
pass_ctrl() { PASS=$((PASS+1)); note "PASS(对照) $*"; }
fail_ctrl() { FAIL=$((FAIL+1)); note "FAIL(对照) $*"; }

TMPROOT=$(mktemp -d /tmp/acc036.XXXXXX)
cleanup() { rm -rf "$TMPROOT"; }
trap cleanup EXIT

TOOLS=(node npm pi cc-switch)

# ── 0. 运行前提 ────────────────────────────────────────────────────
note "0/6 运行前提"
[ "$(id -un)" = "dev" ] && pass "以 dev 用户运行" || fail "当前用户非 dev：$(id -un)"
[ -f /.dockerenv ] && pass "运行于容器内" || fail "未检测到 /.dockerenv，应在容器内执行"

# ── 1. 版本、帮助与 shell 可达性（036 R1–R3、037 R1–R2）────────────
note "1/6 版本、帮助与 shell 可达性"
NODE_VER=$(node --version)       && pass "node --version：$NODE_VER"       || fail "node --version 失败"
NPM_VER=$(npm --version)         && pass "npm --version：$NPM_VER"         || fail "npm --version 失败"
PI_VER=$(pi --version)           && pass "pi --version：$PI_VER"           || fail "pi --version 失败"
CCSW_VER=$(cc-switch --version | awk '{print $NF}') && pass "cc-switch --version：$CCSW_VER" || fail "cc-switch --version 失败"
pi --help >/dev/null 2>&1        && pass "pi --help 退出码 0"              || fail "pi --help 失败"
cc-switch --help >/dev/null 2>&1 && pass "cc-switch --help 退出码 0"       || fail "cc-switch --help 失败"

# 三种 shell（非登录非交互 / 登录 / 交互）× 四工具：必须全部解析到镜像系统路径。
for shell_desc in "非登录:bash -c" "登录:bash -lc" "交互:bash -ic"; do
  label=${shell_desc%%:*}; runner=${shell_desc#*:}
  ok=1
  for t in "${TOOLS[@]}"; do
    p=$($runner "command -v $t" 2>/dev/null)
    case "$p" in
      /usr/local/bin/*) ;;
      *) ok=0; fail "$label shell 中 $t 解析异常：${p:-未找到}" ;;
    esac
  done
  [ "$ok" = 1 ] && pass "$label shell 中 node/npm/pi/cc-switch 全部解析到 /usr/local/bin"
done
# 对照：PATH 里没有这些程序时必须被发现（证明版本/解析断言能发现缺失软件）。
if env PATH=/usr/bin:/bin bash -c 'command -v pi' >/dev/null 2>&1; then
  fail_ctrl "缺程序对照：清空 PATH 后仍找到 pi，检测无效"
else
  pass_ctrl "缺程序对照：清空 PATH 后 pi 按预期消失并被检测到"
fi

# ── 2. 镜像入口优先（036 §2「旧启动器」风险）────────────────────────
note "2/6 镜像入口与卷内阴影"
PI_REAL=$(realpath "$(command -v pi)")
case "$PI_REAL" in
  /usr/local/lib/node_modules/*) pass "pi 实体来自镜像 npm 全局目录：$PI_REAL" ;;
  *) fail "pi 实体不在镜像内：$PI_REAL" ;;
esac
NPM_REAL=$(realpath "$(command -v npm)")
case "$NPM_REAL" in
  /usr/local/lib/node_modules/npm/*) pass "npm 实体来自镜像：$NPM_REAL" ;;
  *) fail "npm 实体不在镜像内：$NPM_REAL" ;;
esac
for t in node cc-switch; do
  [ "$(realpath "$(command -v $t)")" = "/usr/local/bin/$t" ] \
    && pass "$t 实体为 /usr/local/bin/$t" || fail "$t 实体异常：$(realpath "$(command -v $t)")"
done
# 卷内遗留程序不得生效：数据卷路径不能出现在 PATH 中（命中与否由第 1 节三
# 种 shell 的解析结果决定，此处守住前提）；遗留文件本身只记录不判失败。
if echo "$PATH" | tr ':' '\n' | grep -qE '^/home/dev/(\.pi|\.cc-switch)(/|$)'; then
  fail "PATH 含数据卷路径，卷内旧程序可能被命中"
else
  pass "PATH 无数据卷路径，卷内遗留程序不会被命中"
  legacy=$(find /home/dev/.pi /home/dev/.cc-switch -type f -perm -u+x \
           \( -name node -o -name npm -o -name pi -o -name cc-switch \) 2>/dev/null | head -5)
  if [ -n "$legacy" ]; then
    note "信息：卷内存在遗留可执行（不在 PATH、不生效）：$(echo $legacy | tr '\n' ' ')"
  fi
fi
# 对照：卷路径优先级检测——假启动器放进 PATH 更靠前的目录必须被发现。
FAKEBIN="$TMPROOT/fakebin"; mkdir -p "$FAKEBIN"; printf '#!/bin/sh\necho fake\n' > "$FAKEBIN/pi"; chmod +x "$FAKEBIN/pi"
if env PATH="$FAKEBIN:$PATH" bash -c 'command -v pi' 2>/dev/null | grep -q '^'"$FAKEBIN"'/pi$'; then
  pass_ctrl "阴影对照：PATH 前置假 pi 被检测器定位（证明阴影检查有效）"
else
  fail_ctrl "阴影对照：PATH 前置假 pi 未被定位到"
fi

# ── 3. 持久挂载与 dev 可写（036 R4–R5、037 R3）──────────────────────
note "3/6 持久挂载与 dev 可写"
for d in /home/dev/.pi /home/dev/.cc-switch; do
  name=${d##*/}; name=${name#.}
  if mountpoint -q "$d"; then
    pass "$d 为独立挂载点"
  else
    fail "$d 不是挂载点"
  fi
  # 挂载目标身份：经 /workspace（宿主项目 bind）在 .container-state/<name> 写
  # 唯一标记，必须出现在挂载路径下——9p 视角 /proc/mounts 源只显示盘符，身份
  # 只能靠同目录内容证明（/workspace/.container-state 是宿主持久目录本体）。
  marker="acc036-mount-id-$$-$(date +%s)"
  if echo "acc036" > "/workspace/.container-state/$name/$marker" \
     && [ "$(cat "$d/$marker" 2>/dev/null)" = "acc036" ]; then
    pass "$d 与宿主 .container-state/$name 为同一目录（跨路径标记一致）"
  else
    fail "$d 未指向宿主 .container-state/$name"
  fi
  rm -f "/workspace/.container-state/$name/$marker" "$d/$marker" 2>/dev/null
  wmarker="$d/.acc036-write-marker"
  if echo "acc036 $(date -Iseconds)" > "$wmarker" && [ "$(cat "$wmarker")" ] && rm "$wmarker"; then
    pass "dev 可在 $d 写入并读回（标记文件已清理）"
  else
    fail "dev 无法在 $d 完成写入读回"
  fi
done
# 对照：普通目录必须被判定为"非预期挂载"（证明能发现丢失挂载）。
PLAIN="$TMPROOT/plain-dir"; mkdir -p "$PLAIN"
if mountpoint -q "$PLAIN" 2>/dev/null; then
  fail_ctrl "缺挂载对照：普通目录被误判为挂载点"
else
  pass_ctrl "缺挂载对照：普通目录按预期被判定为非挂载点"
fi

# ── 4. CC-Switch 数据库初始化与重开（037 §5 场景 4 的容器内部分）────
note "4/6 CC-Switch 数据库"
FRESH_CC="$TMPROOT/fresh-cc-home"
mkdir -p "$FRESH_CC"
if env -i HOME="$FRESH_CC" PATH="$PATH" TERM=xterm cc-switch provider list >/dev/null 2>&1 \
   && [ -f "$FRESH_CC/.cc-switch/cc-switch.db" ]; then
  pass "隔离 HOME 全新初始化：真实 CLI 生成 cc-switch.db"
else
  fail "隔离 HOME 全新初始化失败"
fi
CCDB="$FRESH_CC/.cc-switch/cc-switch.db"
if [ -f "$CCDB" ]; then
  q=$(python3 - "$CCDB" <<'PY'
import sqlite3, sys
con = sqlite3.connect(sys.argv[1])
print(con.execute("PRAGMA quick_check").fetchone()[0])
PY
)
  [ "$q" = "ok" ] && pass "全新数据库 quick_check = ok" || fail "全新数据库 quick_check：$q"
  echo "acc036 fake marker $(date -Iseconds)" > "$FRESH_CC/.cc-switch/acc036-marker"
  if env -i HOME="$FRESH_CC" PATH="$PATH" TERM=xterm cc-switch provider list >/dev/null 2>&1 \
     && [ "$(cat "$FRESH_CC/.cc-switch/acc036-marker" 2>/dev/null)" ]; then
    pass "数据库可重新打开，假标记保留"
  else
    fail "重开后数据库不可用或假标记丢失"
  fi
fi
# 真实库健康检查：只复制到临时目录后检查，不写真实库。
cp /home/dev/.cc-switch/cc-switch.db /home/dev/.cc-switch/cc-switch.db-wal \
   /home/dev/.cc-switch/cc-switch.db-shm "$TMPROOT/" 2>/dev/null
rq=$(python3 - "$TMPROOT/cc-switch.db" <<'PY'
import sqlite3, sys
con = sqlite3.connect(sys.argv[1])
print(con.execute("PRAGMA quick_check").fetchone()[0])
PY
)
[ "$rq" = "ok" ] && pass "真实库（只读副本）quick_check = ok" || fail "真实库副本 quick_check：$rq"

# ── 5. Pi 全新目录首次写入（036 §5 场景 3 的容器内部分）─────────────
note "5/6 Pi 首次写入"
FRESH_PI="$TMPROOT/fresh-pi-home"
mkdir -p "$FRESH_PI"
if env -i HOME="$FRESH_PI" PATH="$PATH" TERM=xterm pi list >/dev/null 2>&1 \
   && [ -d "$FRESH_PI/.pi/agent" ]; then
  pass "隔离 HOME 首次运行：pi 创建自身数据目录 .pi/agent"
else
  fail "pi 首次运行未创建 .pi/agent"
fi
[ -n "$(ls -A /home/dev/.pi/agent 2>/dev/null)" ] \
  && pass "真实 .pi/agent 非空（既有会话数据在位）" \
  || fail "真实 .pi/agent 为空"

# ── 6. 数据跨容器重建保留（036 §5 场景 5）───────────────────────────
note "6/6 数据跨重建保留"
# 容器启动时刻 = 内核 btime + PID1 starttime（tick）。持久文件 mtime 早于它，
# 说明数据来自挂载而非本容器可写层——本容器即 2026-10-30 宿主重建的产物，
# 其可写层已丢弃（旧迁移备份目录消失佐证），数据只能经 .container-state 挂载存续。
BOOT=$(python3 - <<'PY'
import time
btime = int(next(l.split()[1] for l in open('/proc/stat') if l.startswith('btime ')))
fields = open('/proc/1/stat').read().rsplit(')', 1)[1].split()
print(btime + int(fields[19]) // 100)
PY
)
note "本次容器启动时刻：$(date -d @$BOOT -Iseconds)"
# cc-switch：以稳定数据库文件的 mtime 证明先于本容器存在。
m=$(stat -c %Y /home/dev/.cc-switch/cc-switch.db)
if [ "$m" -lt "$BOOT" ]; then
  pass "cc-switch.db mtime（$(date -d @$m -Iseconds)）早于容器启动 → 来自挂载，跨重建保留"
else
  fail "cc-switch.db mtime 不早于容器启动，无法证明跨重建保留"
fi
# pi：目录 mtime 会随会话写入更新，改用树内最早文件证明数据先于容器。
oldest=$(find /home/dev/.pi -type f -printf '%T@\n' 2>/dev/null | sort -n | head -1 | cut -d. -f1)
if [ -n "$oldest" ] && [ "$oldest" -lt "$BOOT" ]; then
  pass ".pi 树内最早文件（$(date -d @$oldest -Iseconds)）早于容器启动 → 既有数据跨重建保留（现共 $(find /home/dev/.pi -type f | wc -l) 个文件）"
else
  fail ".pi 树内不存在早于容器启动的文件"
fi
# 对照：此刻新建文件的 mtime 必须晚于容器启动（证明时间断言方向正确）。
NOWFILE="$TMPROOT/nowfile"; touch "$NOWFILE"; nm=$(stat -c %Y "$NOWFILE")
if [ "$nm" -ge "$BOOT" ]; then
  pass_ctrl "时钟对照：新建文件 mtime 晚于容器启动（断言方向有效）"
else
  fail_ctrl "时钟对照：新建文件 mtime 异常早于容器启动"
fi

# ── 汇总 ───────────────────────────────────────────────────────────
note "──────────────────────────────────"
note "镜像标识：node $NODE_VER / npm $NPM_VER / pi $PI_VER / cc-switch $CCSW_VER；pi 实体 $PI_REAL"
note "结果：PASS=$PASS FAIL=$FAIL（含对照用例）"
note "未覆盖（需宿主 docker，见 README）：临时容器/临时卷停启与删除重建、docker pull + --no-cache 刷新构建演示"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
