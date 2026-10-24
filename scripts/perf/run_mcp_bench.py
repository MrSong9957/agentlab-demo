#!/usr/bin/env python3
"""022 MCP 基线运行器：目录梯度×场景，驱动 mcp_bench example。
停止条件（固定）：单场景墙钟 > 120s 或 RSS 采样失败 > 3 → 该梯度记录为「达限」。
输出：results-mcp/<scenario>-t<tools>-s<schema>.txt（每行毫秒）＋ meta 行。
主进程与 MCP 子进程内存：每样本轮询两者 VmHWM（子进程 pid 从 mcp.json 无法预知——
经 /proc 扫描 cmd 含 mcp_server.py 的直属子进程）。"""
import json, os, shutil, statistics, subprocess, sys, threading, time

H = "/tmp/perf-harness"
BENCH = "/tmp/perf-target-new/release/examples/mcp_bench"
FIXTURE = f"{H}/mcp_server.py"
OUT = f"{H}/results-mcp"
os.makedirs(OUT, exist_ok=True)

def write_config(user, tools, schema, scenario, delay_ms=0):
    os.makedirs(user, exist_ok=True)
    os.chmod(user, 0o700)
    cfg = {
        "mcp_servers": {"fx": {
            "enabled": True, "command": "python3",
            "args": [FIXTURE, "--tools", str(tools), "--schema-bytes", str(schema),
                     "--scenario", scenario, *(["--delay-ms", str(delay_ms)] if delay_ms else [])],
            "cwd": "/tmp", "startup_timeout_sec": 30, "tool_timeout_sec": 30,
        }}
    }
    with open(f"{user}/mcp.json", "w") as f:
        json.dump(cfg, f)

def child_pids(parent_pid):
    try:
        out = subprocess.check_output(["pgrep", "-P", str(parent_pid)]).decode().split()
    except subprocess.CalledProcessError:
        return []
    return [int(p) for p in out]

def vmhwm(pid):
    try:
        with open(f"/proc/{pid}/status") as f:
            for line in f:
                if line.startswith("VmHWM"):
                    return int(line.split()[1])
    except FileNotFoundError:
        pass
    return 0

def run_scenario(scenario, tools, schema, n=20, delay_ms=0, fx_scenario="echo"):
    tag = f"{scenario}-t{tools}-s{schema}"
    user = f"{H}/muser-{tag}"
    shutil.rmtree(user, ignore_errors=True)
    fx = fx_scenario if scenario != "relist_after_notify" else "notify"
    write_config(user, tools, schema, fx, delay_ms)
    env = dict(os.environ)
    env.update(TCHAT_USER_DIR=user, LC_ALL="C.UTF-8", TZ="UTC")
    t0 = time.monotonic()
    p = subprocess.Popen([BENCH, scenario, str(tools), str(schema), str(n)],
                         stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
    main_peak = 0; child_peak = 0; err_poll = 0
    def sample():
        nonlocal main_peak, child_peak, err_poll
        while p.poll() is None:
            m = vmhwm(p.pid)
            if m > main_peak: main_peak = m
            found = False
            for c in child_pids(p.pid):
                v = vmhwm(c)
                if v:
                    if v > child_peak: child_peak = v
                    found = True
            if not found and time.monotonic() - t0 > 5:
                pass  # 子进程可能已退出（close 场景）
            time.sleep(0.02)
    th = threading.Thread(target=sample); th.start()
    try:
        out, err = p.communicate(timeout=120)
        timed_out = False
    except subprocess.TimeoutExpired:
        p.kill(); out, err = p.communicate(); timed_out = True
    th.join()
    elapsed = time.monotonic() - t0
    samples = [float(x) for x in out.decode().split()]
    with open(f"{OUT}/{tag}.txt", "w") as f:
        f.write("\n".join(f"{s:.3f}" for s in samples))
    with open(f"{OUT}/{tag}-meta.txt", "w") as f:
        json.dump({"scenario": scenario, "tools": tools, "schema": schema, "n": len(samples),
                   "wall_s": round(elapsed, 2), "main_peak_kib": main_peak, "child_peak_kib": child_peak,
                   "timed_out": timed_out, "rc": p.returncode, "stderr_tail": err.decode()[-200:]}, f)
    def pstats(xs):
        if not xs: return "n/a"
        xs = sorted(xs)
        return f"p50={statistics.median(xs):.1f} p95={xs[max(0,int(len(xs)*0.95)-1)]:.1f} min={xs[0]:.1f} max={xs[-1]:.1f} n={len(xs)}"
    print(f"{tag}: {pstats(samples)} mainRss={main_peak/1024:.1f}MiB childRss={child_peak/1024:.1f}MiB wall={elapsed:.1f}s rc={p.returncode}{' TIMEOUT' if timed_out else ''}")
    shutil.rmtree(user, ignore_errors=True)

if __name__ == "__main__":
    # 目录梯度（停止条件：每梯度墙钟 120s 上限内置）
    GRADIENT = [(8, 256), (64, 256), (512, 256), (8, 4096), (64, 4096), (512, 4096)]
    for tools, schema in GRADIENT:
        run_scenario("steady_snapshot", tools, schema)
        run_scenario("call_rtt", tools, schema)
    # 单次性场景（各梯度测一次 discover；取消/关闭用中等梯度）
    for tools, schema in GRADIENT:
        run_scenario("discover_first", tools, schema, n=1)
    for tools, schema in [(64, 256), (512, 4096)]:
        run_scenario("relist_after_notify", tools, schema, n=5)
        run_scenario("close", tools, schema, n=1)
    run_scenario("cancel_inflight", 64, 256, n=10, delay_ms=200)
