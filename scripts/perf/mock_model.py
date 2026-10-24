#!/usr/bin/env python3
"""最小本地 mock 模型服务：POST 返回固定 chat.completion JSON（无工具调用）。"""
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

RESP = json.dumps({"choices": [{"message": {"role": "assistant", "content": "基准回复"}, "finish_reason": "stop"}]})

class H(BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        self.rfile.read(n)
        body = RESP.encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *a):
        pass

port = int(sys.argv[1])
HTTPServer(("127.0.0.1", port), H).serve_forever()
