"""Loopback OpenAI chat-completions server that answers with scripted turns and records each request."""

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def chunk(delta, finish=None):
    return {"id": "mock", "object": "chat.completion.chunk", "created": 0, "model": "mock-model",
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        self.reply(200, "application/json", b'{"data":[{"id":"mock-model"}]}')

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
        item = self.server.mock.next_turn(self.path, body)
        chunks = []
        if "text" in item:
            chunks.append(chunk({"role": "assistant", "content": item["text"]}))
            finish = "stop"
        else:
            for index, tool in enumerate(item["tools"]):
                call = {"index": index, "id": self.server.mock.call_id(), "type": "function",
                        "function": {"name": tool["name"], "arguments": json.dumps(tool["args"])}}
                chunks.append(chunk({"role": "assistant", "tool_calls": [call]}))
            finish = "tool_calls"
        chunks.append(chunk({}, finish))
        usage = {"prompt_tokens": 100, "completion_tokens": 10, "total_tokens": 110}
        if not body.get("stream"):
            message = {"role": "assistant", "content": item.get("text")}
            if "tools" in item:
                message["tool_calls"] = [c["choices"][0]["delta"]["tool_calls"][0] for c in chunks[:-1]]
            out = {"id": "mock", "object": "chat.completion", "model": "mock-model",
                   "choices": [{"index": 0, "message": message, "finish_reason": finish}], "usage": usage}
            self.reply(200, "application/json", json.dumps(out).encode())
            return
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        tail = {"id": "mock", "object": "chat.completion.chunk", "created": 0, "model": "mock-model",
                "choices": [], "usage": usage}
        for part in chunks + [tail]:
            self.wfile.write(f"data: {json.dumps(part)}\n\n".encode())
            self.wfile.flush()
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

    def reply(self, status, kind, data):
        self.send_response(status)
        self.send_header("content-type", kind)
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


class MockOpenAI:
    """`turns` are `{"text": ...}` or `{"tools": [{"name", "args"}]}`; later requests get a fixed text."""

    def __init__(self, turns):
        self.turns = list(turns)
        self.requests = []
        self.calls = 0
        self.lock = threading.Lock()
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.mock = self
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.url = f"http://127.0.0.1:{self.server.server_port}/v1"

    def next_turn(self, path, body):
        with self.lock:
            item = self.turns.pop(0) if self.turns else {"text": "mock: nothing queued"}
            self.requests.append({"path": path, "reply": item, "body": body})
            return item

    def call_id(self):
        with self.lock:
            self.calls += 1
            return f"call_{self.calls}"

    def close(self):
        self.server.shutdown()
        self.server.server_close()
