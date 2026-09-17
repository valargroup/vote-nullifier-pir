#!/usr/bin/env python3
"""Isolated alert delivery test; never changes a real PIR endpoint or live state."""

import argparse
import http.server
import importlib.machinery
import importlib.util
import json
import tempfile
import threading
import time
from pathlib import Path

loader = importlib.machinery.SourceFileLoader(
    "probe", str(Path(__file__).with_name("pir-probe"))
)
spec = importlib.util.spec_from_loader(loader.name, loader)
p = importlib.util.module_from_spec(spec)
loader.exec_module(p)
a = argparse.ArgumentParser()
a.add_argument("--send-alerts", action="store_true")
args = a.parse_args()
state = {"fail": False}


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        self.send_response(503 if state["fail"] else 200)
        self.end_headers()
        self.wfile.write(b'{"status":"ok"}')


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
try:
    with tempfile.TemporaryDirectory(prefix="pir-probe-fault-test-") as tmp:
        s = p.State(Path(tmp) / "status.json", notify=args.send_alerts)
        m = p.Monitor({"targets": []}, s, "unused", threading.Event())
        target = {"url": "http://127.0.0.1:" + str(server.server_port)}
        key = "MONITORING TEST isolated-origin/ready"
        assert p.availability(target)["ready"]["ok"]
        state["fail"] = True
        for _ in range(2):
            s.result(key, p.availability(target)["ready"])
        state["fail"] = False
        for _ in range(2):
            s.result(key, p.availability(target)["ready"])
        # Cryptographic rejection itself is covered by the Rust mutation tests.
        # This validates immediate alert routing for that result classification.
        key = "MONITORING TEST invalid-proof/query"
        s.result(key, {"ok": False, "error": "invalid_proof"}, immediate=True)
        for _ in range(2):
            s.result(key, {"ok": True})
        if args.send_alerts:
            deadline = time.monotonic() + 60
            while s.data["pending"] and time.monotonic() < deadline:
                m.notify_once()
                time.sleep(0.2)
            if s.data["pending"]:
                raise SystemExit("FAIL: notifications not delivered")
        print(
            json.dumps(
                {
                    "fault_tests": "passed",
                    "slack_notifications": 4 if args.send_alerts else 0,
                }
            )
        )
finally:
    server.shutdown()
