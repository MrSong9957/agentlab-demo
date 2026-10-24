#!/usr/bin/env python3
"""022 接入开销 A/B 基线（f662cf6=old vs 4fbf052=new）。
方法固定：预热2＋N=30交替A/B；计时=进程启动→退出（time.monotonic）；
内存=VmHWM 峰值轮询（10ms）；失败=非零退出码计入且样本作废补测（上限3）。
场景：startup（EOF即退）；chat_turn（一个普通用户轮，本地 mock 模型）。"""
import json, os, shutil, statistics, subprocess, sys, time, signal

H = "/tmp/perf-harness"
BIN_OLD = "/tmp/perf-target-old/release/terminal-chat"
BIN_NEW = "/tmp/perf-target-new/release/terminal-chat"
N = 30
MOCK_PORT = 18221

def setup_env(tag):
    user = f"{H}/user-{tag}"; cfg = f"{H}/cfg-{tag}"
    for p in (user, cfg):
        shutil.rmtree(p, ignore_errors=True); os.makedirs(p, exist_ok=True); os.chmod(p, 0o700)
    manual = {"selected_provider": "mock", "selected_model": "mock-1",
              "providers": {"mock": {"base_url": f"http://127.0.0.1:{MOCK_PORT}", "key_ref": "env:PERF_KEY", "protocol": "openai-chat",
                                     "models": {"mock-1": {"id": "mock-1", "context_length": 8192}}}}}
    with open(f"{cfg}/manual.json", "w") as f:
        json.dump(manual, f)
    os.chmod(f"{cfg}/manual.json", 0o600)
    # 预置 origin 授权（非交互发送门）：指纹=sha256(PERF_KEY) 前 16 hex
    import hashlib
    fp = hashlib.sha256(b"perf-fake-key").hexdigest()[:16]
    with open(f"{user}/authorized-origins.json", "w") as f:
        json.dump({"grants": [{"origin": f"http://127.0.0.1:{MOCK_PORT}", "cred_fp": fp, "added_at": 0}]}, f)
    os.chmod(f"{user}/authorized-origins.json", 0o600)
    return user, cfg

def run_once(binary, user, cfg, stdin_text):
    env = dict(os.environ)
    env.update(TCHAT_USER_DIR=user, TCHAT_CONFIG_DIR=cfg, TCHAT_SESSION_TS="20260101T000000",
               TERM="dumb", PERF_KEY="perf-fake-key", LC_ALL="C.UTF-8", TZ="UTC")
    t0 = time.monotonic()
    p = subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL, env=env)
    peak = 0
    import threading
    def sample():
        nonlocal peak
        while p.poll() is None:
            try:
                with open(f"/proc/{p.pid}/status") as f:
                    for line in f:
                        if line.startswith("VmHWM"):
                            v = int(line.split()[1])
                            if v > peak: peak = v
                            break
            except FileNotFoundError:
                return
            time.sleep(0.01)
    th = threading.Thread(target=sample); th.start()
    try:
        p.communicate(stdin_text, timeout=30)
    except subprocess.TimeoutExpired:
        p.kill(); p.wait(); return None, None, "timeout"
    th.join()
    elapsed = time.monotonic() - t0
    return elapsed, peak, ("ok" if p.returncode == 0 else f"rc={p.returncode}")

def bench(scenario, stdin_text):
    rows = {"old": [], "new": []}; rss = {"old": [], "new": []}; fails = {"old": 0, "new": 0}
    bins = {"old": BIN_OLD, "new": BIN_NEW}
    # 预热各 2
    for side in ("old", "new"):
        for _ in range(2):
            u, c = setup_env(f"warm-{side}")
            run_once(bins[side], u, c, stdin_text)
    # 交替采样（补测规则：失败样本作废，失败>3 中止场景）
    for i in range(N):
        for side in ("old", "new"):
            attempt = 0
            while True:
                u, c = setup_env(f"{scenario}-{side}-{i}")
                t, mem, status = run_once(bins[side], u, c, stdin_text)
                if status == "ok":
                    rows[side].append(t); rss[side].append(mem)
                    break
                fails[side] += 1; attempt += 1
                if fails[side] > 3:
                    print(f"ABORT {scenario}/{side}: {status}"); return
    out = f"{H}/results-overhead"
    os.makedirs(out, exist_ok=True)
    for side in ("old", "new"):
        with open(f"{out}/{scenario}-{side}-times.txt", "w") as f:
            f.write("\n".join(f"{x:.6f}" for x in rows[side]))
        with open(f"{out}/{scenario}-{side}-rss.txt", "w") as f:
            f.write("\n".join(str(x) for x in rss[side]))
    def stats(xs):
        xs = sorted(xs)
        p50 = statistics.median(xs)
        p95 = xs[max(0, int(len(xs) * 0.95) - 1)]
        return p50, p95, min(xs), max(xs)
    for side in ("old", "new"):
        p50, p95, lo, hi = stats(rows[side])
        m = statistics.median(rss[side])
        print(f"{scenario}/{side}: p50={p50*1000:.1f}ms p95={p95*1000:.1f}ms min={lo*1000:.1f} max={hi*1000:.1f} rss_med={m/1024:.1f}MiB fails={fails[side]} n={len(rows[side])}")

if __name__ == "__main__":
    mock = subprocess.Popen(["python3", f"{H}/mock_model.py", str(MOCK_PORT)])
    time.sleep(0.3)
    try:
        bench("startup", b"")            # EOF：仅启动横幅+审计后退出
        bench("chat_turn", "基准输入\n".encode())
    finally:
        mock.terminate(); mock.wait()
