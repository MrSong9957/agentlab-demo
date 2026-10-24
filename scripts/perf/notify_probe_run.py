#!/usr/bin/env python3
"""通知生效轮次对照探针运行器：基线 vs 候选二进制 × NOTIFY_DELAY_MS 扫描。
每次运行：全新 user dir＋mcp.json（notify_after_ms 场景）→ 跑 probe_notify_turn
（prepare×5，轮间1ms）→ 读 record list 次数 → 生效轮次判定。
每格重复 5 次取众数与全分布（受控同步：夹具 timer 精度 ±1ms，延迟命中窗口
由扫描覆盖；不做 sleep-竞态赌运气）。"""
import json, os, shutil, subprocess, sys

BASE_BIN = "/tmp/probe-base-target/release/examples/mcp_bench"
CAND_BIN = "/tmp/probe-cand-target/release/examples/mcp_bench"
FIX = "/workspace/scripts/perf/mcp_server.py"
H = "/tmp/perf-harness"

def once2(binary, delay_ms, tag, gap_ms):
    user = f"{H}/probe-user-{tag}"
    rec = f"{H}/probe-rec-{tag}.jsonl"
    shutil.rmtree(user, ignore_errors=True)
    try: os.remove(rec)
    except FileNotFoundError: pass
    os.makedirs(user, exist_ok=True); os.chmod(user, 0o700)
    cfg = {"mcp_servers": {"fx": {"enabled": True, "command": "python3",
        "args": [FIX, "--tools", "8", "--schema-bytes", "256",
                 "--scenario", "notify_after_ms", "--record", rec],
        "cwd": "/tmp", "startup_timeout_sec": 30, "tool_timeout_sec": 30,
        "env_vars": ["NOTIFY_DELAY_MS"]}}}
    with open(f"{user}/mcp.json", "w") as f: json.dump(cfg, f)
    env = dict(os.environ)
    env.update(TCHAT_USER_DIR=user, NOTIFY_DELAY_MS=str(delay_ms),
               NOTIFY_GAP_MS=str(gap_ms), LC_ALL="C.UTF-8")
    import time as _t
    t0 = _t.time()
    p = subprocess.run([binary, "probe_notify_turn", "8", "256", "6"],
                       capture_output=True, env=env, timeout=120)
    events = []
    try:
        for line in open(rec):
            e = json.loads(line)
            events.append(round((e["t"] - t0) * 1000, 1))
    except FileNotFoundError:
        pass
    shutil.rmtree(user, ignore_errors=True)
    return events

def once(binary, delay_ms, tag):
    user = f"{H}/probe-user-{tag}"
    rec = f"{H}/probe-rec-{tag}.jsonl"
    shutil.rmtree(user, ignore_errors=True)
    try: os.remove(rec)
    except FileNotFoundError: pass
    os.makedirs(user, exist_ok=True); os.chmod(user, 0o700)
    cfg = {"mcp_servers": {"fx": {"enabled": True, "command": "python3",
        "args": [FIX, "--tools", "8", "--schema-bytes", "256",
                 "--scenario", "notify_after_ms", "--record", rec],
        "cwd": "/tmp", "startup_timeout_sec": 30, "tool_timeout_sec": 30,
        "env_vars": ["NOTIFY_DELAY_MS"]}}}
    with open(f"{user}/mcp.json", "w") as f: json.dump(cfg, f)
    env = dict(os.environ)
    env.update(TCHAT_USER_DIR=user, NOTIFY_DELAY_MS=str(delay_ms), NOTIFY_GAP_MS=os.environ.get("NOTIFY_GAP_MS","30"), LC_ALL="C.UTF-8")
    p = subprocess.run([binary, "probe_notify_turn", "8", "256", "5"],
                       capture_output=True, env=env, timeout=60)
    lists = 0
    try:
        for line in open(rec):
            if json.loads(line).get("kind") == "list": lists += 1
    except FileNotFoundError:
        pass
    shutil.rmtree(user, ignore_errors=True)
    return lists

def sweep(name, binary):
    print(f"== {name}")
    for delay in (0, 5, 10, 20, 25, 30, 35, 40, 50, 60, 75, 90, 110, 140):
        results = [once(binary, delay, f"{name}-{delay}-{i}") for i in range(5)]
        from collections import Counter
        print(f"  delay={delay:>3}ms lists分布={sorted(Counter(results).items())}  (2=下一轮生效, >2=更晚)")

GAP = int(os.environ.get("NOTIFY_GAP_MS", "200"))

def sweep2(name, binary):
    print(f"== {name} gap={GAP}ms（第 k 轮窗口≈k*{GAP}ms；list 时间戳判定生效轮次）")
    for delay in (210, 230, 250, 270, 300, 350, 410, 470, 530):
        # delay 相对首 list（≈第 1 轮内）；通知到达落在第 ceil((delay-首list偏移)/GAP)+1 轮附近
        evs = [once2(binary, delay, f"{name}-g{GAP}-{delay}-{i}", GAP) for i in range(3)]
        print(f"  delay={delay:>3}ms list时刻(ms)={evs}")

if __name__ == "__main__":
    sweep2("基线4fbf052", BASE_BIN)
    sweep2("候选方案A", CAND_BIN)
