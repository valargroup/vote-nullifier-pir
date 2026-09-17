import importlib.machinery
import importlib.util
import os
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest.mock import patch

loader = importlib.machinery.SourceFileLoader(
    "probe", str(Path(__file__).with_name("pir-probe"))
)
spec = importlib.util.spec_from_loader(loader.name, loader)
p = importlib.util.module_from_spec(spec)
loader.exec_module(p)


class ProbeTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name) / "status.json"
        self.state = p.State(self.path)

    def test_failure_recovery_restart_and_dedup(self):
        s = self.state
        s.result("primary/ready", {"ok": False, "error": "timeout"}, 100)
        self.assertEqual(s.data["pending"], [])
        s.result("primary/ready", {"ok": False, "error": "timeout"}, 160)
        s = p.State(self.path)
        s.result("primary/ready", {"ok": False, "error": "timeout"}, 220)
        self.assertEqual([x["kind"] for x in s.data["pending"]], ["ALERT"])
        s.result("primary/ready", {"ok": True}, 280)
        s.result("primary/ready", {"ok": True}, 340)
        self.assertEqual([x["kind"] for x in s.data["pending"]], ["ALERT", "RECOVERY"])

    def test_correctness_alert_is_immediate_and_reminders_bounded(self):
        s = self.state
        s.result("primary/query", {"ok": False, "error": "invalid_proof"}, 100, True)
        s.result("primary/query", {"ok": False, "error": "invalid_proof"}, 2000, True)
        s.result("primary/query", {"ok": False, "error": "invalid_proof"}, 4000, True)
        self.assertEqual([x["kind"] for x in s.data["pending"]], ["ALERT", "REMINDER"])

    def test_stale_unknown_and_future_apm(self):
        body = b'<div class="updated"><span></span>Updated 2s ago &middot; 2026-09-17T04:00:00+00:00</div>'
        now = p.datetime.datetime.fromisoformat("2026-09-17T04:00:00+00:00").timestamp()
        self.assertTrue(p.apm_result(body, now + 10)["ok"])
        self.assertEqual(p.apm_result(body, now + 100)["error"], "apm_scrape_stale")
        self.assertFalse(p.apm_result(body, now - 100)["ok"])
        self.assertEqual(
            p.apm_result(b"<div>changed</div>", now)["error"], "apm_format_changed"
        )

    def test_transport_and_malformed_readiness(self):
        with patch.object(p, "http", return_value=(200, b"{")):
            self.assertFalse(p.availability({"url": "https://test"})["ready"]["ok"])
        with patch.object(p, "http", side_effect=TimeoutError):
            self.assertFalse(p.availability({"url": "https://test"})["ready"]["ok"])
        with patch.object(p, "http", return_value=(200, b'{"status":"ok"}')):
            self.assertTrue(p.availability({"url": "https://test"})["ready"]["ok"])

    def test_hung_query_is_killed_and_worker_errors_are_safe(self):
        binary = Path(self.tmp.name) / "worker"
        binary.write_text("#!/usr/bin/env python3\nimport time\ntime.sleep(5)\n")
        binary.chmod(0o700)
        start = time.monotonic()
        r = p.query_once(str(binary), "https://test", {}, 0.1)
        self.assertEqual(r["error"], "query_timeout")
        self.assertLess(time.monotonic() - start, 2)
        binary.write_text('#!/usr/bin/env python3\nprint("not JSON")\n')
        self.assertEqual(
            p.query_once(str(binary), "https://test", {})["error"],
            "query_worker_failed",
        )

    def monitor(self):
        with patch.dict(
            os.environ, {"PIR_PROBE_SLACK_WEBHOOK_URL": "https://example.test/secret"}
        ):
            return p.Monitor({"targets": []}, self.state, "worker", threading.Event())

    def test_slack_retry_preserves_recovery_order(self):
        self.state.result(
            "primary/query",
            {"ok": False, "error": "invalid_proof"},
            time.time() - 200,
            True,
        )
        self.state.result("primary/query", {"ok": True})
        self.state.result("primary/query", {"ok": True})
        m = self.monitor()
        with patch.object(p, "http", side_effect=TimeoutError):
            m.notify_once()
        self.assertEqual(len(self.state.data["pending"]), 2)
        self.assertEqual(self.state.data["pending"][0]["attempts"], 1)
        with patch.object(p, "http") as send:
            m.notify_once()
            send.assert_not_called()
        self.state.data["pending"][0]["next"] = 0
        with patch.object(p, "http", return_value=(200, b"ok")):
            m.notify_once()
            m.notify_once()
        self.assertEqual(self.state.data["pending"], [])

    def test_no_false_heartbeat_for_missing_config(self):
        m = self.monitor()
        m.c["config_url"] = "https://test"
        with (
            patch.object(p, "load_layout", side_effect=ValueError),
            patch.object(p, "heartbeat") as hb,
        ):
            m.query_cycle()
            hb.assert_not_called()
        self.assertNotIn("query", self.state.data["cycles"])

    def test_target_failures_still_complete_monitor_cycle(self):
        m = self.monitor()
        m.c.update(
            config_url="https://config",
            targets=[{"name": "one", "url": "https://one"}],
            query_timeout_seconds=1,
        )
        m.hb["query"] = "https://example.test/heartbeat"
        with (
            patch.object(p, "load_layout", return_value={}),
            patch.object(p, "validate_layout"),
            patch.object(
                p, "query_once", return_value={"ok": False, "error": "timeout"}
            ),
            patch.object(p, "heartbeat", return_value=True) as hb,
        ):
            m.query_cycle()
            hb.assert_called_once()
        self.assertIn("query", self.state.data["cycles"])

    def test_config_rejects_credentials_and_duplicates(self):
        for targets in [
            [{"name": "x", "url": "https://user:secret@test"}],
            [
                {"name": "x", "url": "https://test"},
                {"name": "x", "url": "https://other"},
            ],
        ]:
            with self.assertRaises(ValueError):
                p.validate_config({"targets": targets})

    def test_slow_http_response_has_a_wall_clock_deadline(self):
        import http.server

        class Slow(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                time.sleep(2)

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Slow)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            start = time.monotonic()
            with self.assertRaises(p.subprocess.TimeoutExpired):
                p.http("http://127.0.0.1:" + str(server.server_port), timeout=0.2)
            self.assertLess(time.monotonic() - start, 1)
        finally:
            server.shutdown()
            server.server_close()

    def test_invalid_layout_is_one_configuration_incident(self):
        m = self.monitor()
        m.c.update(
            config_url="https://config", targets=[{"name": "one", "url": "https://one"}]
        )
        with (
            patch.object(p, "load_layout", return_value={}),
            patch.object(p, "validate_layout", side_effect=ValueError),
            patch.object(p, "query_once") as query,
        ):
            m.query_cycle()
            query.assert_not_called()
        self.assertEqual(list(self.state.data["checks"]), ["monitor/config"])

    def test_corrupt_state_fails_closed(self):
        self.path.write_text("{")
        with self.assertRaises(ValueError):
            p.State(self.path)


if __name__ == "__main__":
    unittest.main()
