#!/usr/bin/env bash
# 033 部署验证——在重建后的共享 dev 容器内执行（bash /workspace/.devcontainer/verify-033-deploy.sh）。
# 覆盖：gitleaks 恢复、bwrap 与开关注入、产品干跑横幅、三型沙箱集成（真实分支，
# 含 PTY/取消/超时/PID ns）、独立假凭据遮蔽实验、E2E（本地 mock）、.env 元数据核对。
# 口径：先验证持久开关与默认启动行为（compose 注入态原样运行，不清除、不补设）；
# 三型沙箱集成同样在默认注入态运行（用例内部 set/remove 自控开关，互斥串行）；
# 仅普通模式回归（e2e_acceptance）单独 env -u TCHAT_SANDBOX 清除开关。
# 退出码：0=全部通过。.env 全程只读元数据（size/mtime），不读正文。
set -u
cd /workspace
PASS=0; FAIL=0
ok()  { echo "PASS: $*"; PASS=$((PASS+1)); }
bad() { echo "FAIL: $*"; FAIL=$((FAIL+1)); }

ENV_BEFORE=$(stat -c '%s %Y' /workspace/.env 2>/dev/null || echo "missing")
echo "== 0. .env 元数据（验证前）: size mtime = $ENV_BEFORE"

echo "== 1. gitleaks 恢复（/home/dev/.local 不在卷内，重建后需重装） =="
if command -v gitleaks >/dev/null 2>&1; then
  ok "gitleaks 已存在: $(gitleaks version)"
else
  mkdir -p /tmp/033-gl && cd /tmp/033-gl
  if curl -fsSL --retry 3 --retry-all-errors -O https://github.com/gitleaks/gitleaks/releases/download/v8.30.1/gitleaks_8.30.1_linux_x64.tar.gz \
     && curl -fsSL --retry 3 --retry-all-errors -O https://github.com/gitleaks/gitleaks/releases/download/v8.30.1/gitleaks_8.30.1_checksums.txt \
     && grep 'linux_x64.tar.gz$' gitleaks_8.30.1_checksums.txt | sha256sum -c - \
     && tar xzf gitleaks_8.30.1_linux_x64.tar.gz gitleaks \
     && mkdir -p /home/dev/.local/bin && mv gitleaks /home/dev/.local/bin/; then
    ok "gitleaks v8.30.1 重装（checksum 校验通过）"
  else
    bad "gitleaks 重装失败（校验不过须排查来源，不得跳过）"
  fi
  cd /workspace
fi

echo "== 2. bubblewrap 与开关注入 =="
if command -v bwrap >/dev/null 2>&1; then
  ok "bwrap: $(bwrap --version)"
else
  bad "bwrap 缺失（镜像构建未生效）"
fi
if [ "$(printenv TCHAT_SANDBOX)" = "1" ]; then
  ok "TCHAT_SANDBOX=1 来自 compose environment（无手工 export）"
else
  bad "TCHAT_SANDBOX 未注入（当前值=[${TCHAT_SANDBOX:-<unset>}]）"
fi

echo "== 3. 产品干跑（REPL 启动横幅应报告 bwrap 可用；EOF 即退出） =="
DRY=$(echo "" | timeout 90 cargo run --quiet 2>&1 || true)
if echo "$DRY" | grep -qi "bubblewrap"; then
  ok "启动横幅提及 bubblewrap: $(echo "$DRY" | grep -i "bubblewrap" | head -1)"
else
  bad "启动输出未见 bubblewrap 报告（前几行: $(echo "$DRY" | head -3 | tr '\n' ' ')）"
fi

echo "== 4. 三型沙箱集成（真实分支；默认注入态运行，用例内部自控开关；exec 含管道 failIfUnavailable 与 PTY 超时组杀） =="
if timeout 600 cargo test --test exec_sandbox --test mcp_stdio_sandbox --test apply_patch_sandbox > /tmp/033-verify-sandbox.log 2>&1; then
  ok "exec_sandbox + mcp_stdio_sandbox + apply_patch_sandbox 全绿（$(grep -c 'test result: ok' /tmp/033-verify-sandbox.log) 个目标）"
else
  bad "沙箱集成失败，日志 /tmp/033-verify-sandbox.log"
fi

echo "== 5. 假凭据遮蔽（独立 tmp 夹具；产品同参 bwrap 子集；遮蔽证据=正文不可得——open EACCES〔userns 内非特权 bind 被内核强制 nodev，2026-10-27 容器实测〕或读到 0 字节，与产品 A2 stat 级口径一致） =="
T=$(mktemp -d)
printf 'FAKE-SECRET-033-DEPLOY' > "$T/.env"
echo "plain-ok" > "$T/plain.txt"
IN=$(timeout 60 bwrap \
  --ro-bind / / \
  --bind "$T" "$T" \
  --dev /dev \
  --proc /proc \
  --unshare-user --unshare-pid --unshare-ipc --as-pid-1 \
  --ro-bind /dev/null "$T/.env" \
  -- /bin/sh -c '
    if s=$(wc -c < "'"$T"'/.env" 2>/dev/null); then echo "size=$s"; else echo "OPEN-DENIED"; fi
    if cat "'"$T"'/.env" >/dev/null 2>&1; then echo "CAT-LEAK"; else echo "CAT-DENIED"; fi
    echo "pid1=$$"
    if [ "$(cat "'"$T"'/plain.txt")" = "plain-ok" ] && echo retry >> "'"$T"'/plain.txt"; then
      echo "PLAIN-RW-OK"
    else
      echo "PLAIN-BROKEN"
    fi' 2>&1)
if echo "$IN" | grep -q "CAT-DENIED" \
   && { echo "$IN" | grep -q "OPEN-DENIED" || echo "$IN" | grep -q "size=0"; } \
   && echo "$IN" | grep -q "pid1=1" && echo "$IN" | grep -q "PLAIN-RW-OK" \
   && grep -q "FAKE-SECRET-033-DEPLOY" "$T/.env"; then
  ok "沙箱内 .env 正文不可得（open 被拒/零字节）且 cat 被拒（外部仍含标记）、pid1=载荷进程、普通文件读写正常"
else
  bad "遮蔽实验异常: 外部=[$(wc -c < "$T/.env")B] 内部=[$(echo "$IN" | tr '\n' ' ')]"
fi
rm -rf "$T"

echo "== 6. 普通模式回归 E2E（本地 mock；单独清除开关 env -u TCHAT_SANDBOX；主流程与端点冒烟，无真实外部调用） =="
if env -u TCHAT_SANDBOX timeout 900 cargo test --test e2e_acceptance > /tmp/033-verify-e2e.log 2>&1; then
  ok "e2e_acceptance 通过（本地 mock）"
else
  bad "e2e_acceptance 失败，日志 /tmp/033-verify-e2e.log"
fi

echo "== 7. .env 元数据（验证后，须与验证前一致） =="
ENV_AFTER=$(stat -c '%s %Y' /workspace/.env 2>/dev/null || echo "missing")
if [ "$ENV_BEFORE" = "$ENV_AFTER" ]; then
  ok ".env 元数据未变: $ENV_AFTER"
else
  bad ".env 元数据变化: 前=[$ENV_BEFORE] 后=[$ENV_AFTER]"
fi

echo
echo "==== 验证汇总: PASS=$PASS FAIL=$FAIL ===="
[ "$FAIL" -eq 0 ]
