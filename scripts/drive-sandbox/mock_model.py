"""Scripted Anthropic Messages endpoint on loopback (port in argv[1]) for the Viewer's drive sandbox: text
`ok`; `MOCK-TOOL` calls Bash `touch mock-tool.txt` (once per such prompt); `MOCK-SLOW` streams ~8 s."""
import json, sys, time, itertools
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
seq = itertools.count()
class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def _json(self, code, v):
        b = json.dumps(v).encode()
        self.send_response(code); self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
        if "count_tokens" in self.path: return self._json(200, {"input_tokens": 10})
        if not self.path.startswith("/v1/messages"): return self._json(404, {"type": "error", "error": {"type": "not_found_error", "message": "no"}})
        msgs = req.get("messages", [])
        def blocks(role):
            out = []
            for m in msgs:
                if m.get("role") != role: continue
                c = m.get("content")
                out += [{"type": "text", "text": c}] if isinstance(c, str) else (c or [])
            return out
        user, asst = blocks("user"), blocks("assistant")
        asked = lambda w: sum(1 for b in user if w in (b.get("text") or ""))
        called = lambda n: sum(1 for b in asst if b.get("type") == "tool_use" and b.get("name") == n)
        has_bash = any(t.get("name") == "Bash" for t in req.get("tools", []))
        tool = None
        if has_bash and asked("MOCK-TOOL") > called("Bash"):
            tool = (f"toolu_py{next(seq):04d}", "Bash", {"command": "touch mock-tool.txt", "description": "Create mock-tool.txt"})
        last = user[-1].get("text", "") if user else ""
        words, gap = (["slow "] * 16, 0.5) if "MOCK-SLOW" in last else (["ok"], 0)
        model = req.get("model", "mock"); stop = "tool_use" if tool else "end_turn"
        msg = {"id": "msg_mock", "type": "message", "role": "assistant", "model": model, "content": [], "stop_reason": None, "stop_sequence": None, "usage": {"input_tokens": 10, "output_tokens": 3}}
        if not req.get("stream"):
            msg["content"] = [{"type": "tool_use", "id": tool[0], "name": tool[1], "input": tool[2]}] if tool else [{"type": "text", "text": "".join(words)}]
            msg["stop_reason"] = stop; return self._json(200, msg)
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.end_headers()
        def ev(name, v):
            self.wfile.write(f"event: {name}\ndata: {json.dumps(v)}\n\n".encode()); self.wfile.flush()
        ev("message_start", {"type": "message_start", "message": msg})
        if tool:
            ev("content_block_start", {"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": tool[0], "name": tool[1], "input": {}}})
            ev("content_block_delta", {"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": json.dumps(tool[2])}})
        else:
            ev("content_block_start", {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})
            for w in words:
                time.sleep(gap); ev("content_block_delta", {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": w}})
        ev("content_block_stop", {"type": "content_block_stop", "index": 0})
        ev("message_delta", {"type": "message_delta", "delta": {"stop_reason": stop, "stop_sequence": None}, "usage": {"output_tokens": 3}})
        ev("message_stop", {"type": "message_stop"})
ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
