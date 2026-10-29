#!/usr/bin/env python3
"""022 stdio MCP 服务器夹具（本地受控，固定行为）。

用法：python3 mcp_stdio_server.py --scenario NAME [--record FILE] [--delay MS]

行为按 --scenario 选择；--record 把 argv、cwd、环境白名单键值、每次
tools/call 的原始名字与参数追加写入 JSONL（测试断言副作用用）。
stdout 只输出协议（newline-delimited JSON-RPC）；stderr 仅供洪流场景。
"""
import argparse
import json
import os
import sys
import threading
import time

PROTO_VERSION = "2025-11-25"


def record(path, payload):
    if not path:
        return
    with open(path, "a", encoding="utf-8") as f:
        f.write(json.dumps(payload, ensure_ascii=False) + "\n")


def snap_env():
    # 只快照白名单相关键 + 显式引用的测试键，避免把真实环境写进记录。
    keys = ["HOME", "PATH", "SHELL", "USER", "LANG", "TERM", "TMPDIR", "TZ",
            "TCHAT_MCP_TOKEN_A", "TCHAT_MCP_MISSING_VAR"]
    return {k: os.environ.get(k) for k in keys if k in os.environ}


class Writer:
    def __init__(self):
        self.lock = threading.Lock()

    def send(self, payload):
        line = json.dumps(payload, ensure_ascii=False)
        with self.lock:
            sys.stdout.write(line + "\n")
            sys.stdout.flush()


def init_result(req, tools_capability=True, resources_capability=False):
    caps = {}
    if tools_capability:
        caps["tools"] = {"listChanged": True}
    if resources_capability:
        caps["resources"] = {"listChanged": True}
    return {
        "jsonrpc": "2.0",
        "id": req["id"],
        "result": {
            "protocolVersion": req["params"].get("protocolVersion", PROTO_VERSION),
            "capabilities": caps,
            "serverInfo": {"name": "tchat-fixture", "version": "0.1"},
        },
    }


def echo_tool():
    return {
        "name": "echo",
        "description": "Echo the given text back.",
        "inputSchema": {
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"],
        },
    }


def call_result(w, req, content, structured=None, is_error=None, meta=None, result_type=None):
    result = {"content": content}
    if structured is not None:
        result["structuredContent"] = structured
    if is_error is not None:
        result["isError"] = is_error
    if meta is not None:
        result["_meta"] = meta
    if result_type is not None:
        result["resultType"] = result_type
    w.send({"jsonrpc": "2.0", "id": req["id"], "result": result})


def text_block(t):
    return {"type": "text", "text": t}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--scenario", required=True)
    ap.add_argument("--record", default="")
    ap.add_argument("--delay-ms", type=int, default=0)
    args, _unknown = ap.parse_known_args()  # 额外 argv（元字符注入测试）按字面忽略
    if args.scenario == "stderr_secret":
        token = os.environ.get("TCHAT_MCP_TOKEN_A", "")
        threading.Thread(target=_stderr_flood, args=(token,), daemon=True).start()
    w = Writer()
    rec = args.record
    record(rec, {"kind": "startup", "argv": sys.argv, "cwd": os.getcwd(),
                 "env": snap_env(), "pid": os.getpid()})

    initialized = False

    def handle_call(req):
        name = req["params"]["name"]
        arguments = req["params"].get("arguments") or {}
        record(rec, {"kind": "call", "name": name, "arguments": arguments})
        s = args.scenario
        if args.delay_ms:
            time.sleep(args.delay_ms / 1000.0)
        if s == "echo":
            call_result(w, req, [text_block(f"echo:{arguments.get('text', '')}")])
        elif s in ("searchable", "searchable_1300", "searchable_budget"):
            call_result(w, req, [text_block(f"called:{name}")])
        elif s == "polluted_result":
            token = os.environ.get("TCHAT_MCP_TOKEN_A", "")
            call_result(w, req, [text_block(f"result with {token} inside")])
        elif s == "content_types":
            call_result(w, req, [
                text_block("第一段"),
                {"type": "image", "data": "aGk=", "mimeType": "image/png"},
                {"type": "audio", "data": "aGk=", "mimeType": "audio/wav"},
                {"type": "resource_link", "uri": "file:///tmp/x", "name": "x"},
                {"type": "resource", "resource": {"uri": "file:///tmp/y",
                                                  "text": "内容", "mimeType": "text/plain"}},
            ])
        elif s == "structured_null":
            # structuredContent 显式 null（与缺省字段相区分）。
            w.send({"jsonrpc": "2.0", "id": req["id"], "result":
                    {"content": [text_block("ok")], "structuredContent": None}})
        elif s == "is_error":
            call_result(w, req, [text_block("服务端业务失败")], is_error=True)
        elif s == "meta":
            call_result(w, req, [text_block("ok")], meta={"k": "v"})
        elif s == "unknown_content":
            call_result(w, req, [{"type": "mystery", "x": 1}])
        elif s == "input_required":
            call_result(w, req, [text_block("需要输入")], result_type="input_required")
        elif s == "task_result":
            call_result(w, req, [text_block("任务")], result_type="task")
        elif s == "write_then_die":
            # 副作用：写固定文件后进程退出（A12：副作用保留、无自动重放）。
            path = arguments.get("path", "")
            with open(path, "w", encoding="utf-8") as f:
                f.write(arguments.get("text", ""))
            w.send({"jsonrpc": "2.0", "id": req["id"], "result":
                    {"content": [text_block("written")]}})
            sys.stdout.flush()
            os._exit(0)
        elif s == "reverse_sampling":
            w.send({"jsonrpc": "2.0", "id": "srv-1", "method": "sampling/createMessage",
                    "params": {"messages": [], "maxTokens": 16}})
            time.sleep(0.5)
            call_result(w, req, [text_block("done")])
        elif s == "reverse_roots":
            w.send({"jsonrpc": "2.0", "id": "srv-1", "method": "roots/list"})
            time.sleep(0.5)
            call_result(w, req, [text_block("done")])
        elif s == "reverse_elicitation":
            w.send({"jsonrpc": "2.0", "id": "srv-1", "method": "elicitation/create",
                    "params": {"message": "give", "requestedSchema": {}}})
            time.sleep(0.5)
            call_result(w, req, [text_block("done")])
        elif s == "oversize_line":
            huge = "x" * (9 * 1024 * 1024)
            w.send({"jsonrpc": "2.0", "id": req["id"], "result":
                    {"content": [text_block(huge)]}})
        elif s == "notify_list_changed":
            w.send({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})
            call_result(w, req, [text_block("ok")])
        elif s == "notify_twice":
            w.send({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})
            call_result(w, req, [text_block("ok")])
        elif s == "protocol_error_call":
            w.send({"jsonrpc": "2.0", "id": req["id"], "error":
                    {"code": -32601, "message": "tool not found"}})
        else:
            call_result(w, req, [text_block(f"echo:{name}")])

    def list_page(cursor=None, tools=None, next_cursor=None):
        result = {"tools": tools}
        if next_cursor is not None:
            result["nextCursor"] = next_cursor
        w.send({"jsonrpc": "2.0", "id": req_id_for_list(cursor),
                "result": result})

    list_requests = []

    def req_id_for_list(_cursor):
        return list_requests[-1] if list_requests else None

    for raw in sys.stdin:
        raw = raw.strip()
        if not raw:
            continue
        try:
            msg = json.loads(raw)
        except json.JSONDecodeError:
            continue
        method = msg.get("method", "")
        mid = msg.get("id")
        s = args.scenario
        if method == "initialize":
            if s == "hang":
                # 初始化握手挂起：启动总时限测试用（不回应）。
                time.sleep(120)
            elif s == "init_no_initialize":
                w.send({"jsonrpc": "2.0", "id": mid, "result":
                        {"protocolVersion": "2026-07-28",
                         "capabilities": {"tools": {}},
                         "serverInfo": {"name": "tchat-fixture", "version": "0.1"}}})
            elif s == "tools_and_resources":
                # 023：两者兼有。
                w.send(init_result(msg, tools_capability=True,
                                   resources_capability=True))
            elif s.startswith("resources_"):
                # 023：资源分页／容量／错误场景——仅资源服务器。
                w.send(init_result(msg, tools_capability=False,
                                   resources_capability=True))
            else:
                caps = s != "no_tools_capability"
                w.send(init_result(msg, tools_capability=caps))
        elif method == "notifications/initialized":
            initialized = True
        elif method == "tools/list":
            if s == "protocol_error_list":
                w.send({"jsonrpc": "2.0", "id": mid, "error":
                        {"code": -32000, "message": "list failed"}})
                continue
            list_requests.append(mid)
            record(rec, {"kind": "list"})
            if s == "notify_on_list":
                # 每次 tools/list **响应之后** 30ms 再主动发目录变化通知：确保
                # 通知在宿主该轮发现基准（请求前取得）之后到达且在轮时间窗内
                # 受理（测试「无近期调用时收到主动通知」的受控同步）。
                def _notify_later():
                    w.send({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})
                threading.Timer(0.03, _notify_later).start()
            page = len(list_requests)
            if s == "notify_twice" and page >= 2:
                # 重列期间再次通知：过期标记必须保留到下一轮（A13）。
                w.send({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})
            if s == "exit_after_list":
                list_page(mid, tools=[echo_tool()])
                sys.stdout.flush()
                os._exit(0)
            if s == "bad_json":
                sys.stdout.write("this is not json\n")
                sys.stdout.flush()
                time.sleep(5)
                continue
            if s == "init_no_initialize":
                pass
            if s == "echo":
                list_page(mid, tools=[echo_tool()])
            elif s == "empty_catalog":
                list_page(mid, tools=[])
            elif s == "paged2":
                if page == 1:
                    list_page(mid, tools=[echo_tool()], next_cursor="page-2")
                else:
                    list_page(mid, tools=[{"name": "second",
                                           "description": "s",
                                           "inputSchema": {"type": "object"}}])
            elif s == "empty_cursor_page":
                # 空字符串游标是有效游标：第一页返回 ""，第二页返回工具并结束。
                if page == 1:
                    w.send({"jsonrpc": "2.0", "id": mid, "result":
                            {"tools": [echo_tool()], "nextCursor": ""}})
                else:
                    list_page(mid, tools=[{"name": "after_empty",
                                           "description": "s",
                                           "inputSchema": {"type": "object"}}])
            elif s == "paged101":
                # 第 101 页仍有游标 → 超过 100 页上限。
                list_page(mid, tools=[echo_tool()], next_cursor=f"p{page}")
            elif s == "items_over":
                # 一次性 2049 项 → 超过目录上限。
                tools = [{"name": f"t{i}", "inputSchema": {"type": "object"}}
                         for i in range(2049)]
                list_page(mid, tools=tools)
            elif s == "cursor_loop":
                list_page(mid, tools=[echo_tool()], next_cursor="loop")
            elif s == "cursor_64k":
                list_page(mid, tools=[echo_tool()], next_cursor="c" * (64 * 1024 + 1))
            elif s == "polluted_definition":
                token = os.environ.get("TCHAT_MCP_TOKEN_A", "")
                list_page(mid, tools=[
                    {"name": "leaky", "description": f"token is {token}",
                     "inputSchema": {"type": "object"}},
                    {"name": "clean", "description": "fine",
                     "inputSchema": {"type": "object"}},
                ])
            elif s == "stderr_secret":
                list_page(mid, tools=[echo_tool()])
            elif s == "tools_129":
                tools = [{"name": f"t{i}", "inputSchema": {"type": "object"}}
                         for i in range(129)]
                list_page(mid, tools=tools)
            elif s == "same_name_tools":
                list_page(mid, tools=[
                    {"name": "dup", "inputSchema": {"type": "object"}},
                    {"name": "dup", "inputSchema": {"type": "object"}},
                ])
            elif s == "unicode_tools":
                list_page(mid, tools=[
                    {"name": "查询", "inputSchema": {"type": "object"}},
                    {"name": "a__b", "inputSchema": {"type": "object"}},
                    {"name": "x" * 70, "inputSchema": {"type": "object"}},
                ])
            elif s == "searchable":
                # 030：可搜索目录——用途词（email/weather/translate）可命中的描述。
                list_page(mid, tools=[
                    {"name": "send_mail",
                     "description": "Send an email message to a recipient",
                     "inputSchema": {"type": "object",
                                     "properties": {"to": {"type": "string"},
                                                    "body": {"type": "string"}}}},
                    {"name": "weather_lookup",
                     "description": "Look up the weather forecast for a city",
                     "inputSchema": {"type": "object",
                                     "properties": {"city": {"type": "string"}}}},
                    {"name": "translate_note",
                     "description": "Translate a text note between languages",
                     "inputSchema": {"type": "object",
                                     "properties": {"text": {"type": "string"},
                                                    "target": {"type": "string"}}}},
                ])
            elif s == "searchable_budget":
                # 030：声明预算边界——5 组×26 项，组词互不重叠（130 项 ≤2048）。
                groups = ["alpha", "beta", "gamma", "delta", "epsilon"]
                tools = []
                for gi, g in enumerate(groups):
                    for j in range(26):
                        tools.append({
                            "name": f"{g}_{j:02d}",
                            "description": f"bulk {g} helper widget",
                            "inputSchema": {"type": "object"},
                        })
                list_page(mid, tools=tools)
            elif s == "searchable_1300":
                # 030：跨服务器目录总量超限（两台 ×1300＝2600＞2048）。
                tools = [{"name": f"t{i}",
                          "description": f"bulk fixture tool number {i}",
                          "inputSchema": {"type": "object"}}
                         for i in range(1300)]
                list_page(mid, tools=tools)
            elif s == "bad_schema_tool":
                list_page(mid, tools=[
                    {"name": "ok", "inputSchema": {"type": "object"}},
                    {"name": "root_string", "inputSchema": {"type": "string"}},
                    {"name": "props_array", "inputSchema": {"type": "object",
                                                            "properties": []}},
                ])
            else:
                list_page(mid, tools=[echo_tool()])
        elif method == "tools/call":
            handle_call(msg)
        elif method == "resources/list":
            params = msg.get("params") or {}
            cursor = params.get("cursor")
            record(rec, {"kind": "list_resources", "params": params})
            if s == "resources_error":
                w.send({"jsonrpc": "2.0", "id": mid, "error":
                        {"code": -32000, "message": "list resources failed"}})
            elif s == "resources_hang_list":
                time.sleep(120)
            elif s == "resources_polluted":
                token = os.environ.get("TCHAT_MCP_TOKEN_A", "")
                w.send({"jsonrpc": "2.0", "id": mid, "result": {"resources": [
                    {"uri": "tchat://dirty/x", "name": "x",
                     "description": f"leak {token} here"},
                ]}})
            elif s == "resources_malformed":
                # 畸形响应：resources 非数组（SDK 反序列化失败路径）。
                w.send({"jsonrpc": "2.0", "id": mid, "result":
                        {"resources": "not-an-array"}})
            elif s == "resources_paged":
                if cursor is None:
                    w.send({"jsonrpc": "2.0", "id": mid, "result": {"resources": [
                        {"uri": "tchat://p/one", "name": "one"},
                    ], "nextCursor": "page-2"}})
                elif cursor == "page-2":
                    w.send({"jsonrpc": "2.0", "id": mid, "result": {"resources": [
                        {"uri": "tchat://p/two", "name": "two"},
                    ]}})
                else:
                    w.send({"jsonrpc": "2.0", "id": mid, "result": {"resources": []}})
            elif s == "resources_empty_cursor":
                # 空字符串游标是有效游标：第一页返回 ""，第二页结束。
                if cursor is None:
                    w.send({"jsonrpc": "2.0", "id": mid, "result": {"resources": [
                        {"uri": "tchat://e/first", "name": "first"},
                    ], "nextCursor": ""}})
                else:
                    w.send({"jsonrpc": "2.0", "id": mid, "result": {"resources": [
                        {"uri": "tchat://e/second", "name": "second"},
                    ]}})
            elif s == "resources_cursor_space":
                # 游标带首尾空白：原样传递，不能 trim。
                if cursor is None:
                    w.send({"jsonrpc": "2.0", "id": mid, "result": {"resources": [
                        {"uri": "tchat://s/first", "name": "first"},
                    ], "nextCursor": " spaced "}})
                else:
                    w.send({"jsonrpc": "2.0", "id": mid, "result": {"resources": [
                        {"uri": "tchat://s/second", "name": "second"},
                    ]}})
            elif s == "resources_loop":
                w.send({"jsonrpc": "2.0", "id": mid, "result": {"resources": [
                    {"uri": "tchat://l/x", "name": "x"},
                ], "nextCursor": "loop"}})
            elif s == "resources_101":
                page = 1 if cursor is None else int(cursor[1:]) + 1
                w.send({"jsonrpc": "2.0", "id": mid, "result": {"resources": [
                    {"uri": f"tchat://h/{page}", "name": "h"},
                ], "nextCursor": f"p{page}"}})
            elif s == "resources_items_over":
                resources = [{"uri": f"tchat://o/{i}", "name": "o",
                              "mimeType": "text/plain"} for i in range(2049)]
                w.send({"jsonrpc": "2.0", "id": mid, "result":
                        {"resources": resources}})
            elif s == "resources_items_paged_over":
                if cursor is None:
                    resources = [{"uri": f"tchat://po/a{i}", "name": "a"}
                                 for i in range(1024)]
                    w.send({"jsonrpc": "2.0", "id": mid, "result":
                            {"resources": resources, "nextCursor": "p2"}})
                else:
                    resources = [{"uri": f"tchat://po/b{i}", "name": "b"}
                                 for i in range(1025)]
                    w.send({"jsonrpc": "2.0", "id": mid, "result":
                            {"resources": resources}})
            elif s == "resources_cursor_64k":
                w.send({"jsonrpc": "2.0", "id": mid, "result": {"resources": [
                    {"uri": "tchat://c/x", "name": "x"},
                ], "nextCursor": "c" * (64 * 1024 + 1)}})
            elif s == "resources_big":
                # 单页 ~3MiB（低于 8MiB 传输上限），三页累计超 8MiB 预算。
                # resources/list 的资源项没有 text 字段（正文属 contents），
                # 大文本放 description。
                page = 1 if cursor is None else int(cursor[1:]) + 1
                huge = "x" * (3 * 1024 * 1024)
                result = {"resources": [
                    {"uri": f"tchat://big/{page}", "name": "big",
                     "mimeType": "text/plain", "description": huge},
                ]}
                if page < 3:
                    result["nextCursor"] = f"p{page}"
                w.send({"jsonrpc": "2.0", "id": mid, "result": result})
            else:
                w.send({"jsonrpc": "2.0", "id": mid, "result": {"resources": [
                    {"uri": "tchat://guide/hello", "name": "hello",
                     "description": "问候资源", "mimeType": "text/plain"},
                    {"uri": "tchat://guide/config", "name": "config",
                     "mimeType": "application/json"},
                ]}})
        elif method == "resources/templates/list":
            record(rec, {"kind": "list_templates",
                         "params": msg.get("params") or {}})
            w.send({"jsonrpc": "2.0", "id": mid, "result": {"resourceTemplates": [
                {"uriTemplate": "tchat://guide/notes/{id}", "name": "note",
                 "description": "按 ID 取笔记", "mimeType": "text/markdown"},
                {"uriTemplate": "tchat://guide/img/{name}", "name": "img",
                 "mimeType": "image/png"},
            ]}})
        elif method == "resources/read":
            params = msg.get("params") or {}
            uri = params.get("uri", "")
            record(rec, {"kind": "read_resource", "params": params})
            if s == "resources_slow_read":
                # 迟到结果：指定 URI 1.5s 后正常回复（配合 1s tool_timeout）。
                if uri == "tchat://guide/slow":
                    time.sleep(1.5)
            if uri == "tchat://guide/config":
                w.send({"jsonrpc": "2.0", "id": mid, "result": {"contents": [
                    {"uri": uri, "mimeType": "application/json",
                     "text": "{\"theme\": \"dark\"}"},
                ], "_meta": {"origin": "fixture"}}})
            elif uri == "tchat://guide/logo":
                import base64
                blob = base64.b64encode(b"\x89PNG-fake-bytes").decode()
                w.send({"jsonrpc": "2.0", "id": mid, "result": {"contents": [
                    {"uri": uri, "mimeType": "image/png", "blob": blob},
                ]}})
            elif uri == "tchat://guide/multi":
                w.send({"jsonrpc": "2.0", "id": mid, "result": {"contents": [
                    {"uri": uri, "mimeType": "text/plain", "text": "第一段"},
                    {"uri": uri + "#part2", "mimeType": "text/plain",
                     "text": "第二段"},
                ]}})
            elif uri == "tchat://guide/polluted":
                token = os.environ.get("TCHAT_MCP_TOKEN_A", "")
                w.send({"jsonrpc": "2.0", "id": mid, "result": {"contents": [
                    {"uri": uri, "text": f"leak {token} here"},
                ]}})
            elif uri == "tchat://guide/polluted_blob":
                import base64
                token = os.environ.get("TCHAT_MCP_TOKEN_A", "")
                blob = base64.b64encode(token.encode()).decode()
                w.send({"jsonrpc": "2.0", "id": mid, "result": {"contents": [
                    {"uri": uri, "blob": blob},
                ]}})
            elif uri == "tchat://guide/polluted_meta":
                token = os.environ.get("TCHAT_MCP_TOKEN_A", "")
                w.send({"jsonrpc": "2.0", "id": mid, "result": {"contents": [
                    {"uri": uri, "text": "clean"},
                ], "_meta": {"note": token}}})
            elif uri == "tchat://guide/hang_read":
                time.sleep(120)
            elif uri == "tchat://guide/huge":
                # 单条消息超过 8 MiB 传输上限（接收有界性实测）。
                w.send({"jsonrpc": "2.0", "id": mid, "result": {"contents": [
                    {"uri": uri, "mimeType": "text/plain",
                     "text": "x" * (9 * 1024 * 1024)},
                ]}})
            else:
                w.send({"jsonrpc": "2.0", "id": mid, "result": {"contents": [
                    {"uri": uri, "mimeType": "text/plain",
                     "text": f"content of {uri}"},
                ]}})
            if s == "resources_die_after_read":
                # 回复已发送，进程退出（023 断连场景）。
                sys.stdout.flush()
                os._exit(0)
        elif mid is not None and method:
            # 未知请求统一 method-not-found（含 ping 之外的探测）。
            if method == "ping":
                w.send({"jsonrpc": "2.0", "id": mid, "result": {}})
            else:
                w.send({"jsonrpc": "2.0", "id": mid, "error":
                        {"code": -32601, "message": "method not found"}})

    # stdin EOF：自然退出。


def _stderr_flood(token):
    while True:
        sys.stderr.write(f"noise before {token} after\n")
        sys.stderr.flush()
        time.sleep(0.05)


if __name__ == "__main__":
    main()
