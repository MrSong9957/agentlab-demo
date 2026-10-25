#!/usr/bin/env python3
"""024 本地性能基准：真实行式 CLI、独立 HTTP 进程、固定历史与载荷。

运行时不得并行构建。--smoke 仅排查夹具，不能用作性能验收。
"""
import argparse
import base64
import hashlib
import json
import math
import multiprocessing as mp
import os
from pathlib import Path
import platform
import pty
import select
import socket
import subprocess
import tarfile
import tempfile
import time
from http.server import BaseHTTPRequestHandler, HTTPServer

PROTOCOLS = ["openai-chat", "openai-responses", "anthropic-messages", "google-interactions", "google-generate-content"]
IMAGE = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg=="
KEY = "fixture-key"


def response(protocol, call_batch):
    calls = [{"id": f"bench-{n}", "type": "function", "function": {"name": "bench_unknown", "arguments": "{}"}} for n in range(32)]
    if protocol == "openai-chat":
        message = {"role": "assistant", "content": None if call_batch else "bench-done"}
        if call_batch:
            message["tool_calls"] = calls
        return {"choices": [{"message": message, "finish_reason": "tool_calls" if call_batch else "stop"}]}
    if protocol == "openai-responses":
        output = [{"type": "function_call", "call_id": c["id"], **c["function"]} for c in calls] if call_batch else [
            {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "bench-done"}]}]
        return {"status": "completed", "output": output}
    if protocol == "anthropic-messages":
        blocks = [{"type": "tool_use", "id": c["id"], "name": c["function"]["name"], "input": {}} for c in calls] if call_batch else [{"type": "text", "text": "bench-done"}]
        return {"role": "assistant", "type": "message", "content": blocks, "stop_reason": "tool_use" if call_batch else "end_turn"}
    if protocol == "google-interactions":
        steps = [{"type": "function_call", "id": c["id"], "name": c["function"]["name"], "arguments": {}} for c in calls] if call_batch else [{"type": "model_output", "content": [{"type": "text", "text": "bench-done"}]}]
        return {"id": "bench-interaction", "status": "requires_action" if call_batch else "completed", "steps": steps}
    parts = [{"functionCall": {"id": c["id"], "name": c["function"]["name"], "args": {}}} for c in calls] if call_batch else [{"text": "bench-done"}]
    return {"candidates": [{"index": 0, "content": {"role": "model", "parts": parts}, "finishReason": "STOP"}]}


def event(name, data):
    return f"event: {name}\ndata: {json.dumps(data, ensure_ascii=False)}\n\n"


def streamed(protocol, value):
    if protocol == "openai-chat":
        choice = value["choices"][0]
        delta = choice["message"].copy()
        for n, call in enumerate(delta.get("tool_calls", [])):
            call["index"] = n
        return event("message", {"choices": [{"index": 0, "delta": delta, "finish_reason": choice["finish_reason"]}]}) + "data: [DONE]\n\n"
    if protocol == "openai-responses":
        return event("response.completed", {"type": "response.completed", "response": value})
    if protocol == "anthropic-messages":
        opening = {**value, "content": [], "stop_reason": None}
        out = event("message_start", {"type": "message_start", "message": opening})
        for n, block in enumerate(value["content"]):
            out += event("content_block_start", {"type": "content_block_start", "index": n, "content_block": block})
            out += event("content_block_stop", {"type": "content_block_stop", "index": n})
        out += event("message_delta", {"type": "message_delta", "delta": {"stop_reason": value["stop_reason"]}})
        return out + event("message_stop", {"type": "message_stop"})
    if protocol == "google-interactions":
        out = event("interaction.created", {"event_type": "interaction.created", "interaction": {"id": "bench-interaction", "status": "in_progress"}})
        for n, step in enumerate(value["steps"]):
            out += event("step.start", {"event_type": "step.start", "index": n, "step": step})
            out += event("step.stop", {"event_type": "step.stop", "index": n})
        return out + event("interaction.completed", {"event_type": "interaction.completed", "interaction": {"id": "bench-interaction", "status": value["status"]}})
    return event("message", value)


def serve(protocol, kind, stream, pipe):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def setup(self):
            super().setup()
            self.connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

        def log_message(self, *_):
            pass

        def do_POST(self):
            length = int(self.headers["Content-Length"])
            body = json.loads(self.rfile.read(length))
            # 已回传过工具结果的请求必须终止循环；初次 32 项负载只返回普通函数。
            if protocol == "openai-chat":
                has_results = any(m.get("role") == "tool" for m in body["messages"])
            elif protocol == "openai-responses":
                has_results = any(i.get("type") == "function_call_output" for i in body["input"])
            elif protocol == "anthropic-messages":
                has_results = any(p.get("type") == "tool_result" for m in body["messages"] for p in m["content"])
            elif protocol == "google-interactions":
                has_results = any(i.get("type") == "function_result" for i in body["input"])
            else:
                has_results = any("functionResponse" in p for m in body["contents"] for p in m["parts"])
            value = response(protocol, kind == "calls32" and not has_results)
            payload = (streamed(protocol, value) if stream else json.dumps(value, ensure_ascii=False)).encode()
            self.server.requests += 1
            self.server.request_sizes.append(length)
            self.server.response_sizes.append(len(payload))
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream" if stream else "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            self.wfile.flush()

    server = HTTPServer(("127.0.0.1", 0), Handler)
    server.requests = 0
    server.request_sizes = []
    server.response_sizes = []
    server.timeout = 0.1
    pipe.send(server.server_port)
    while not pipe.poll():
        server.handle_request()
    pipe.recv()
    pipe.send({"requests": server.requests, "request_bytes": [min(server.request_sizes), max(server.request_sizes)],
               "response_bytes": [min(server.response_sizes), max(server.response_sizes)]})
    server.server_close()


def measure(binary, protocol, kind, stream, smoke):
    parent, child = mp.Pipe()
    server = mp.Process(target=serve, args=(protocol, kind, stream, child))
    server.start()
    if not parent.poll(10):
        server.terminate(); server.join(); raise RuntimeError("mock 启动超时")
    base_url = f"http://127.0.0.1:{parent.recv()}"
    warmup, count, rounds = (1, 2, 1) if smoke else (20, 200, 3)
    proc = None
    master = None
    try:
        with tempfile.TemporaryDirectory(prefix="tchat-native-bench-") as name:
            root = Path(name); config = root / "config"; user = root / "user"; sessions = user / "sessions"
            config.mkdir(); sessions.mkdir(parents=True)
            model = {"id": "bench-model", "max_output": 256, "input_modalities": ["text", "image"], "supported_params": ["stream"]}
            manual = {"selected_provider": "bench", "selected_model": "bench-model", "providers": {"bench": {"protocol": protocol, "base_url": base_url,
                "key_ref": "env:NATIVE_BENCH_KEY", "request_params": {"stream": stream}, "models": {"bench-model": model}}}}
            (config / "manual.json").write_text(json.dumps(manual))
            (user / "authorized-origins.json").write_text(json.dumps({"grants": [{"origin": base_url, "cred_fp": hashlib.sha256(KEY.encode()).hexdigest()[:16], "added_at": 0}]}))
            saved = '{"v":1}\n'
            if kind == "image":
                saved += json.dumps({"type": "text", "role": "user", "content": "准备后图片", "images": [IMAGE]}, ensure_ascii=False) + "\n"
            (sessions / "bench.jsonl").write_text(saved)
            for path in [config / "manual.json", user / "authorized-origins.json", sessions / "bench.jsonl"]:
                path.chmod(0o600)
            master, slave = pty.openpty()
            env = {"PATH": os.environ.get("PATH", ""), "TERM": "dumb", "TZ": "UTC", "LANG": "C.UTF-8", "NATIVE_BENCH_KEY": KEY,
                "TCHAT_CONFIG_DIR": str(config), "TCHAT_USER_DIR": str(user), "TCHAT_SESSION_TS": "20261025T120000", "TCHAT_SESSION_PID": "24"}
            proc = subprocess.Popen([str(binary)], stdin=slave, stdout=slave, stderr=slave, cwd=root, env=env, start_new_session=True)
            os.close(slave)
            pending = bytearray()

            def until(marker):
                deadline = time.monotonic() + 15
                marker = marker.encode()
                while marker not in pending:
                    if time.monotonic() > deadline or proc.poll() is not None:
                        raise RuntimeError(f"CLI 未到达 {marker!r}: {pending[-2000:]!r}")
                    if select.select([master], [], [], 0.2)[0]:
                        pending.extend(os.read(master, 65536))
                end = pending.index(marker) + len(marker)
                del pending[:end]

            def write(line):
                os.write(master, line.encode() + b"\n")

            def once(has_history):
                write("/resume bench")
                if has_history:
                    until("继续？"); write("y")
                until("处）"); until("你> ")
                # 恢复已释放旧自动文件；删本基准的临时产物，排除撞名次数增长。
                for path in sessions.glob("auto-*.jsonl"):
                    path.unlink()
                started = time.perf_counter_ns()
                write("bench prompt")
                until("Agent：bench-done")
                elapsed = (time.perf_counter_ns() - started) / 1000
                until("你> ")
                return elapsed

            until("你> ")
            for n in range(warmup):
                once(n > 0)
            # 只重置本基准子进程的 RSS 高水位；不改变内存或系统限制。
            Path(f"/proc/{proc.pid}/clear_refs").write_text("5")
            samples = [[once(True) for _ in range(count)] for _ in range(rounds)]
            status = Path(f"/proc/{proc.pid}/status").read_text()
            peak_kib = int(next(line.split()[1] for line in status.splitlines() if line.startswith("VmHWM:")))
            write("exit"); proc.wait(timeout=10)
            if proc.returncode:
                raise RuntimeError(f"CLI exit={proc.returncode}")
            parent.send("stop")
            if not parent.poll(10):
                raise RuntimeError("mock 统计等待超时")
            stats = parent.recv()
            expected = (warmup + count * rounds) * (2 if kind == "calls32" else 1)
            if stats["requests"] != expected:
                raise RuntimeError(f"未走目标负载：{stats['requests']} != {expected}")
            p95 = [sorted(values)[math.ceil(len(values) * .95) - 1] for values in samples]
            return {"case": f"{protocol}/{kind}/{'sse' if stream else 'json'}", "protocol": protocol, "load": kind, "stream": stream,
                "warmup": warmup, "samples_per_round": count, "rounds": rounds, "p95_us": p95, "worst_p95_us": max(p95),
                "peak_rss_kib": peak_kib, "samples_us": samples, **stats}
    finally:
        if proc is not None and proc.poll() is None:
            proc.kill(); proc.wait()
        if master is not None:
            os.close(master)
        server.terminate(); server.join()
        parent.close(); child.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--smoke", action="store_true")
    args = parser.parse_args()
    workspace = Path(__file__).resolve().parents[3]
    manifest = {str(path.relative_to(workspace)): hashlib.sha256(path.read_bytes()).hexdigest()
                for path in sorted((workspace / "src/app/src").rglob("*.rs"))}
    metadata = {"os": platform.platform(), "cpu": next(line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")),
        "memory": next(line for line in Path("/proc/meminfo").read_text().splitlines() if line.startswith("MemTotal:")),
        "rust": subprocess.check_output(["rustc", "-Vv"], text=True).strip(), "build": "debug/locked/offline",
        "cargo_lock_sha256": hashlib.sha256((workspace / "Cargo.lock").read_bytes()).hexdigest(), "concurrency": 1,
        "latency": "真实 CLI 输入至最终文本可见；不含快照恢复；含工具循环和自动保存", "rss": "预热后重置本子进程 VmHWM，再读正式轮峰值",
        "client_control": "同一 CLI／ureq 客户端的 openai-chat 固定响应，本地 mock 不做推理；含 CLI 及协议处理开销", "source_sha256": manifest}
    cases = [(p, k, s) for p in PROTOCOLS for k in ["text", "calls32", "image"] for s in [False, True]]
    if args.smoke:
        cases = [("openai-chat", "text", False), ("google-interactions", "calls32", True), ("google-generate-content", "image", True)]
    rows = []
    for protocol, kind, stream in cases:
        row = measure(args.binary.resolve(), protocol, kind, stream, args.smoke)
        rows.append(row)
        print(f"{row['case']}: p95={row['worst_p95_us']:.1f} us, peak={row['peak_rss_kib']} KiB", flush=True)
    controls = {(r["load"], r["stream"]): r["peak_rss_kib"] for r in rows if r["protocol"] == "openai-chat"}
    for row in rows:
        row["rss_delta_vs_chat_kib"] = row["peak_rss_kib"] - controls.get((row["load"], row["stream"]), row["peak_rss_kib"])
    failed = []
    if args.baseline:
        limits = {r["case"]: r for r in json.loads(args.baseline.read_text())["rows"]}
        for row in rows:
            limit = limits[row["case"]]
            if row["worst_p95_us"] > limit["latency_limit_us"] or row["peak_rss_kib"] > limit["rss_limit_kib"]:
                failed.append(row["case"])
    elif not args.smoke:
        for row in rows:
            row["latency_limit_us"] = math.ceil(row["worst_p95_us"] * 3)
            row["rss_limit_kib"] = row["peak_rss_kib"] * 3
        with tarfile.open(args.output.with_suffix(".source.tar.gz"), "w:gz") as archive:
            for path in [workspace / "Cargo.toml", workspace / "Cargo.lock", workspace / "src/app/Cargo.toml", workspace / "src/app/src"]:
                archive.add(path, arcname=path.relative_to(workspace))
    args.output.write_text(json.dumps({"smoke_only": args.smoke, "metadata": metadata, "rows": rows, "failed": failed}, ensure_ascii=False, indent=2) + "\n")
    if failed:
        raise SystemExit("性能超限：" + ", ".join(failed))


if __name__ == "__main__":
    main()
