import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


MODULE_PATH = Path(__file__).resolve().parents[1] / "scripts" / "planner_bridge.py"
SPEC = importlib.util.spec_from_file_location("planner_bridge", MODULE_PATH)
bridge_module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bridge_module)


class RelayFixture:
    def __init__(self):
        self.token = "first-session-token"
        self.notification = {"id": "test-notification", "task_id": "blocked-task",
                             "payload": {"reason": "repair needed", "planner": {"context": "origin-session"}}}
        self.acknowledged = False
        self.fail_ack = False
        self.incomplete_next = False
        self.registrations = []
        self.bootstrap_count = 0
        fixture = self

        class Handler(BaseHTTPRequestHandler):
            def handle(self):
                try:
                    super().handle()
                except (ConnectionResetError, BrokenPipeError):
                    # The subprocess test explicitly stops its polling client.
                    pass

            def log_message(self, *_):
                pass

            def respond(self, code, value):
                data = json.dumps(value).encode()
                self.send_response(code)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def do_GET(self):
                fixture.bootstrap_count += 1
                self.respond(200, {"token": fixture.token})

            def do_POST(self):
                value = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
                if self.headers.get("x-relay-token") != fixture.token:
                    self.respond(403, {"error": "refresh token"})
                elif self.path.endswith("/register"):
                    fixture.registrations.append(value)
                    self.respond(200, {"planner_id": "receiver", "planner": value})
                elif self.path.endswith("/next"):
                    if fixture.incomplete_next:
                        fixture.incomplete_next = False
                        self.send_response(200)
                        self.send_header("Content-Length", "3000")
                        self.end_headers()
                        self.wfile.write(b'{"notification":')
                        self.close_connection = True
                        return
                    self.respond(200, {"notification": None if fixture.acknowledged else fixture.notification,
                                       "retry_after_ms": 1000})
                elif self.path.endswith("/ack"):
                    if fixture.fail_ack:
                        self.respond(503, {"error": "restart in progress"})
                    else:
                        assert value == {"notification_id": fixture.notification["id"]}
                        fixture.acknowledged = True
                        self.respond(200, {"acknowledged": value["notification_id"], "acknowledged_at": "now"})
                else:
                    self.respond(404, {"error": "unexpected endpoint"})

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.api_base = f"http://127.0.0.1:{self.server.server_port}/api/projects/workspace"

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()


class PlannerBridgeTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        self.fixture = RelayFixture()
        self.handler = self.root / "handler.py"
        self.handler.write_text("""import json, os, pathlib, sys
record = pathlib.Path(sys.argv[1])
events = json.loads(record.read_text()) if record.exists() else []
notification = json.load(sys.stdin)
events.append({'id': notification['id'], 'args': sys.argv[2:],
               'file': json.loads(pathlib.Path(os.environ['RELAY_NOTIFICATION_FILE']).read_text()),
               'context': os.environ['RELAY_PLANNER_CONTEXT'],
               'env_id': os.environ['RELAY_NOTIFICATION_ID']})
record.write_text(json.dumps(events))
sys.exit(7 if len(events) == 1 and os.path.exists(str(record) + '.fail-first') else 0)
""", encoding="utf-8")
        self.record = self.root / "executions.json"
        self.bridges = []

    def tearDown(self):
        for bridge in self.bridges:
            bridge.close()
        self.fixture.close()
        self.directory.cleanup()

    def bridge(self):
        instance = bridge_module.PlannerBridge(
            bridge_module.RelayClient(self.fixture.api_base), "local", "planner", "Local planner",
            "current-session", [sys.executable, str(self.handler), str(self.record),
                                "{notification_id}", "{context}", "{notification_file}"],
            self.root / "state", workspace_default=True, task_ids=["blocked-task"],
        )
        self.bridges.append(instance)
        return instance

    def test_real_handler_receives_original_context_then_acknowledges(self):
        receiver = self.bridge()
        self.assertEqual(receiver.step(), "acknowledged")
        self.assertTrue(self.fixture.acknowledged)
        executed = json.loads(self.record.read_text())
        self.assertEqual(len(executed), 1)
        self.assertEqual(executed[0]["id"], "test-notification")
        self.assertEqual(executed[0]["env_id"], "test-notification")
        self.assertEqual(executed[0]["context"], "origin-session")
        self.assertEqual(executed[0]["args"][:2], ["test-notification", "origin-session"])
        self.assertEqual(executed[0]["file"], self.fixture.notification)
        self.assertTrue(self.fixture.registrations[0]["workspace_default"])
        self.assertEqual(self.fixture.registrations[0]["task_ids"], ["blocked-task"])
        self.assertEqual(receiver.step(), "idle")

    def test_handler_failure_is_retried_and_not_acknowledged(self):
        Path(str(self.record) + ".fail-first").touch()
        receiver = self.bridge()
        self.assertEqual(receiver.step(), "retry")
        self.assertFalse(self.fixture.acknowledged)
        self.assertEqual(receiver.step(), "acknowledged")
        self.assertEqual(len(json.loads(self.record.read_text())), 2)
        attempts = receiver.database.execute("SELECT attempts FROM receipts").fetchone()[0]
        self.assertEqual(attempts, 2)

    def test_successful_receipt_survives_receiver_and_server_restart(self):
        receiver = self.bridge()
        self.fixture.fail_ack = True
        with self.assertRaises(bridge_module.urllib.error.HTTPError):
            receiver.step()
        self.assertFalse(self.fixture.acknowledged)
        self.assertEqual(len(json.loads(self.record.read_text())), 1)
        receiver.close()
        self.bridges.remove(receiver)
        self.fixture.token = "restarted-session-token"
        self.fixture.fail_ack = False
        restarted = self.bridge()
        self.assertEqual(restarted.step(), "acknowledged")
        self.assertEqual(len(json.loads(self.record.read_text())), 1)
        self.assertTrue(self.fixture.acknowledged)

    def test_stale_session_token_refreshes_without_losing_message(self):
        receiver = self.bridge()
        receiver.register()
        self.fixture.token = "new-token"
        self.assertEqual(receiver.step(), "acknowledged")
        self.assertEqual(self.fixture.bootstrap_count, 2)

    def test_receiver_loop_retries_a_connection_closed_mid_response(self):
        self.fixture.incomplete_next = True
        command = [sys.executable, str(self.handler), str(self.record),
                   "{notification_id}", "{context}", "{notification_file}"]
        options = {"creationflags": subprocess.CREATE_NO_WINDOW} if sys.platform == "win32" else {}
        process = subprocess.Popen([
            sys.executable, str(MODULE_PATH), "--api-base", self.fixture.api_base,
            "--source", "local", "--agent-id", "planner", "--context", "current-session",
            "--command-json", json.dumps(command), "--state-dir", str(self.root / "loop-state"),
            "--poll-interval", "0.01", "--retry-delay", "0.01",
        ], stdout=subprocess.PIPE, stderr=subprocess.PIPE, **options)
        try:
            deadline = time.monotonic() + 10
            while not self.fixture.acknowledged and process.poll() is None and time.monotonic() < deadline:
                time.sleep(0.02)
            self.assertTrue(self.fixture.acknowledged, "Receiver stopped instead of retrying a truncated HTTP response")
            self.assertEqual(len(json.loads(self.record.read_text())), 1)
        finally:
            process.terminate()
            process.communicate(timeout=5)

    def test_receiver_state_lock_excludes_a_second_process(self):
        state = self.root / "state"
        state.mkdir()
        with bridge_module.StateLock(state):
            code = """import importlib.util, pathlib, sys
spec=importlib.util.spec_from_file_location('bridge', sys.argv[1])
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
try:
    with m.StateLock(pathlib.Path(sys.argv[2])): pass
except RuntimeError: sys.exit(11)
"""
            process = subprocess.run([sys.executable, "-c", code, str(MODULE_PATH), str(state)], capture_output=True)
            self.assertEqual(process.returncode, 11, process.stderr.decode())


if __name__ == "__main__":
    unittest.main()
