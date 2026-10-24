#!/usr/bin/env python3
"""性能夹具 stdio MCP 服务器：--tools N --schema-bytes B [--scenario name] [--record FILE]。
目录：N 个工具，每个 inputSchema 含一个 pad 字符串属性（约 B 字节）。
场景：echo（快响应）／notify（调用后发目录变化通知）／notify_after_ms
（首 list 应答后延迟 NOTIFY_DELAY_MS 发主动通知——该变量须经 mcp.json
env_vars 显式转交，缺失即报错退出；--record 追加 JSONL 事件含时间戳）。
"""
import argparse, json, os, sys, threading, time

ap = argparse.ArgumentParser()
ap.add_argument("--tools", type=int, default=8)
ap.add_argument("--schema-bytes", type=int, default=256)
ap.add_argument("--scenario", default="echo")
ap.add_argument("--delay-ms", type=int, default=0)
ap.add_argument("--record", default="")
args, _ = ap.parse_known_args()

pad = "p" * max(0, args.schema_bytes - 64)

def tools_list():
    tools = []
    for i in range(args.tools):
        tools.append({
            "name": f"tool_{i:04d}",
            "description": f"perf tool {i}",
            "inputSchema": {
                "type": "object",
                "properties": {"q": {"type": "string"}, "pad": {"type": "string", "description": pad}},
                "required": ["q"],
            },
        })
    return tools

w = sys.stdout
def send(obj):
    w.write(json.dumps(obj, ensure_ascii=False) + "\n")
    w.flush()

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    method = msg.get("method", "")
    mid = msg.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": mid, "result": {
            "protocolVersion": "2025-11-25",
            "capabilities": {"tools": {"listChanged": True}},
            "serverInfo": {"name": "perf-fx", "version": "0.1"}}})
    elif method == "notifications/initialized":
        pass
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": mid, "result": {"tools": tools_list()}})
        if args.record:
            with open(args.record, "a", encoding="utf-8") as f:
                f.write(json.dumps({"kind": "list", "t": time.time()}) + "\n")
        if args.scenario == "notify_after_ms":
            # 语义探针：首次 list 应答后延迟 NOTIFY_DELAY_MS 发主动通知。
            # （该变量必须经 mcp.json env_vars 显式转交；缺失即配置错误，报错退出。）
            delay_raw = os.environ.get("NOTIFY_DELAY_MS")
            if delay_raw is None:
                sys.stderr.write("notify_after_ms 需要 NOTIFY_DELAY_MS（经 env_vars 转交）\n")
                sys.exit(3)
            if not globals().get("_notify_sent"):
                globals()["_notify_sent"] = True
                threading.Timer(
                    float(delay_raw) / 1000.0,
                    lambda: (sys.stdout.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"}) + "\n"), sys.stdout.flush()),
                ).start()
    elif method == "tools/call":
        if args.delay_ms:
            time.sleep(args.delay_ms / 1000.0)
        send({"jsonrpc": "2.0", "id": mid, "result": {
            "content": [{"type": "text", "text": "perf-ok"}], "isError": False}})
        if args.scenario == "notify":
            send({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})
    elif mid is not None:
        send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": "nf"}})
