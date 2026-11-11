#!/usr/bin/env python3
"""Deliver Relay planner notifications to any configured command-line runtime.

The command gets the notification on stdin and through RELAY_NOTIFICATION_FILE.
Exit zero acknowledges receipt; it does not resolve the incident or finish work.
Commands must tolerate redelivery by RELAY_NOTIFICATION_ID after a crash.
"""

import argparse
import contextlib
import hashlib
import http.client
import json
import os
from pathlib import Path
import signal
import sqlite3
import subprocess
import sys
import time
import urllib.error
import urllib.request


class RelayClient:
    def __init__(self, api_base, http_timeout=30):
        self.api_base = api_base.rstrip("/")
        self.http_timeout = http_timeout
        self.token = None

    def refresh_token(self):
        request = urllib.request.Request(self.api_base + "/bootstrap")
        with urllib.request.urlopen(request, timeout=self.http_timeout) as response:
            self.token = json.load(response)["token"]

    def post(self, path, body):
        if self.token is None:
            self.refresh_token()
        for refresh in [False, True]:
            request = urllib.request.Request(
                self.api_base + path,
                data=json.dumps(body).encode("utf-8"),
                headers={"Content-Type": "application/json", "x-relay-token": self.token},
                method="POST",
            )
            try:
                with urllib.request.urlopen(request, timeout=self.http_timeout) as response:
                    return json.load(response)
            except urllib.error.HTTPError as error:
                if not refresh and error.code in (401, 403):
                    self.refresh_token()
                    continue
                raise


class StateLock:
    """Only one receiver may execute commands for a given local state directory."""

    def __init__(self, directory):
        self.path = directory / "receiver.lock"
        self.file = None

    def __enter__(self):
        self.file = self.path.open("a+b")
        self.file.seek(0)
        try:
            if os.name == "nt":
                import msvcrt
                msvcrt.locking(self.file.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(self.file.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            self.file.close()
            raise RuntimeError(f"Another planner receiver is using {self.path.parent}") from None
        return self

    def __exit__(self, *_):
        if self.file is not None:
            if os.name == "nt":
                import msvcrt
                self.file.seek(0)
                msvcrt.locking(self.file.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                import fcntl
                fcntl.flock(self.file.fileno(), fcntl.LOCK_UN)
            self.file.close()


def stop_handler(process):
    if process.poll() is not None:
        return
    if os.name == "nt":
        # Only the child tree created by this receiver is terminated.
        subprocess.run(
            ["taskkill", "/PID", str(process.pid), "/T", "/F"],
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            creationflags=subprocess.CREATE_NO_WINDOW, check=False,
        )
        if process.poll() is None:
            process.terminate()
    else:
        with contextlib.suppress(ProcessLookupError):
            os.killpg(process.pid, signal.SIGTERM)
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        if os.name == "nt":
            process.kill()
        else:
            with contextlib.suppress(ProcessLookupError):
                os.killpg(process.pid, signal.SIGKILL)
        process.wait()


class PlannerBridge:
    def __init__(self, client, source, agent_id, name, context, command, state_dir, workspace_default=False, task_ids=None):
        if not isinstance(command, list) or not command or not all(isinstance(x, str) for x in command):
            raise ValueError("command must be a nonempty JSON array of strings")
        self.client = client
        self.registration = {"source": source, "agent_id": agent_id, "name": name, "context": context,
                             "workspace_default": workspace_default, "task_ids": task_ids or []}
        self.command = command
        self.context = context if isinstance(context, str) else json.dumps(context, separators=(",", ":"))
        self.state_dir = Path(state_dir).resolve()
        self.state_dir.mkdir(parents=True, exist_ok=True)
        self.database = sqlite3.connect(self.state_dir / "receipts.sqlite")
        self.database.execute("PRAGMA journal_mode=WAL")
        self.database.execute("PRAGMA synchronous=FULL")
        self.database.execute("""CREATE TABLE IF NOT EXISTS receipts (
            notification_id TEXT PRIMARY KEY,
            attempts INTEGER NOT NULL DEFAULT 0,
            completed_at REAL,
            acknowledged_at REAL
        )""")
        self.database.commit()
        self.planner_id = None

    def close(self):
        self.database.close()

    def register(self):
        registration = self.client.post("/planners/register", self.registration)
        self.planner_id = registration["planner_id"]

    def _run_handler(self, notification):
        notification_id = notification["id"]
        context_value = notification.get("payload", {}).get("planner", {}).get("context", self.context)
        context = context_value if isinstance(context_value, str) else json.dumps(context_value, separators=(",", ":"))
        # Hash untrusted event IDs before constructing paths.
        key = hashlib.sha256(notification_id.encode("utf-8")).hexdigest()
        folder = self.state_dir / "notifications" / key
        folder.mkdir(parents=True, exist_ok=True)
        notification_file = folder / "notification.json"
        temporary = folder / "notification.tmp"
        temporary.write_text(json.dumps(notification, ensure_ascii=False, indent=2), encoding="utf-8")
        temporary.replace(notification_file)
        with self.database:
            self.database.execute(
                """INSERT INTO receipts(notification_id, attempts) VALUES(?, 1)
                   ON CONFLICT(notification_id) DO UPDATE SET attempts=attempts+1""",
                (notification_id,),
            )
        attempt = self.database.execute(
            "SELECT attempts FROM receipts WHERE notification_id=?", (notification_id,)
        ).fetchone()[0]
        replacements = {
            "{notification_file}": str(notification_file),
            "{notification_id}": notification_id,
            "{context}": context,
        }
        args = []
        for argument in self.command:
            for marker, value in replacements.items():
                argument = argument.replace(marker, value)
            args.append(argument)
        environment = os.environ.copy()
        environment.update({
            "RELAY_NOTIFICATION_FILE": str(notification_file),
            "RELAY_NOTIFICATION_ID": notification_id,
            "RELAY_PLANNER_CONTEXT": context,
            "RELAY_API_BASE": self.client.api_base,
            "RELAY_PLANNER_ID": self.planner_id,
        })
        options = {"start_new_session": True} if os.name != "nt" else {
            "creationflags": subprocess.CREATE_NEW_PROCESS_GROUP | subprocess.CREATE_NO_WINDOW,
        }
        with notification_file.open("rb") as stdin, (folder / f"{attempt}.stdout.log").open("wb") as stdout, (folder / f"{attempt}.stderr.log").open("wb") as stderr:
            process = subprocess.Popen(args, stdin=stdin, stdout=stdout, stderr=stderr, env=environment, **options)
            try:
                # Intentionally no execution timeout: a long repair is work,
                # not a delivery failure. Explicit shutdown still stops it.
                code = process.wait()
            except BaseException:
                stop_handler(process)
                raise
        if code != 0:
            print(f"Planner handler exited {code}; notification {notification_id} remains pending. Logs: {folder}", file=sys.stderr, flush=True)
            return False
        with self.database:
            self.database.execute("UPDATE receipts SET completed_at=? WHERE notification_id=?", (time.time(), notification_id))
        return True

    def step(self):
        if self.planner_id is None:
            self.register()
        response = self.client.post(f"/planners/{self.planner_id}/next", {})
        notification = response.get("notification")
        if notification is None:
            return "idle"
        notification_id = notification["id"]
        receipt = self.database.execute(
            "SELECT completed_at FROM receipts WHERE notification_id=?", (notification_id,)
        ).fetchone()
        if (receipt is None or receipt[0] is None) and not self._run_handler(notification):
            return "retry"
        self.client.post(f"/planners/{self.planner_id}/ack", {"notification_id": notification_id})
        with self.database:
            self.database.execute("UPDATE receipts SET acknowledged_at=? WHERE notification_id=?", (time.time(), notification_id))
        return "acknowledged"


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--api-base", required=True, help="Workspace API base, e.g. http://127.0.0.1:4242/api/projects/ID")
    parser.add_argument("--source", required=True, help="Provider/runtime identity, e.g. cursor, claude, codex, local")
    parser.add_argument("--agent-id", required=True, help="Stable identity of the originating planning agent")
    parser.add_argument("--name", default="Planner callback receiver")
    parser.add_argument("--context", default="", help="Provider session/thread reference or JSON context")
    parser.add_argument("--workspace-default", action="store_true", help="Receive blockers for workspace tasks with no explicit originating planner")
    parser.add_argument("--task-id", action="append", default=[], help="Bind an existing task to this planner; repeat for multiple tasks")
    parser.add_argument("--command-json", required=True, help="JSON argument array for a command that receives the callback")
    parser.add_argument("--state-dir", type=Path, help="Durable receipt/log directory; reuse it on restart")
    parser.add_argument("--poll-interval", type=float, default=1)
    parser.add_argument("--retry-delay", type=float, default=5)
    parser.add_argument("--http-timeout", type=float, default=30, help="Per HTTP request timeout; errors are retried, never abandoned")
    args = parser.parse_args(argv)
    try:
        command = json.loads(args.command_json)
        context = json.loads(args.context)
    except json.JSONDecodeError:
        # Plain session identifiers are valid context; command JSON is not.
        context = args.context
        try:
            command = json.loads(args.command_json)
        except json.JSONDecodeError as error:
            parser.error(f"Invalid --command-json: {error.msg}")
    if min(args.poll_interval, args.retry_delay, args.http_timeout) <= 0:
        parser.error("Polling, retry, and HTTP request intervals must be positive")
    identity = json.dumps([args.api_base.rstrip("/"), args.source, args.agent_id])
    state_dir = args.state_dir or Path.home() / ".relay" / "planner-bridges" / hashlib.sha256(identity.encode()).hexdigest()[:24]
    try:
        bridge = PlannerBridge(RelayClient(args.api_base, args.http_timeout), args.source, args.agent_id, args.name, context, command, state_dir, args.workspace_default, args.task_id)
    except ValueError as error:
        parser.error(str(error))
    try:
        with StateLock(bridge.state_dir):
            print(f"Planner receiver running; state: {bridge.state_dir}", flush=True)
            while True:
                try:
                    result = bridge.step()
                    time.sleep(args.retry_delay if result == "retry" else args.poll_interval)
                except (OSError, http.client.HTTPException, ValueError, KeyError, sqlite3.Error) as error:
                    # Re-registration is idempotent, including after a server
                    # restart/reset. Successful local receipts remain durable.
                    bridge.planner_id = None
                    print(f"Planner delivery will retry: {error}", file=sys.stderr, flush=True)
                    time.sleep(args.retry_delay)
    except KeyboardInterrupt:
        print("Planner receiver stopped.", file=sys.stderr, flush=True)
    finally:
        bridge.close()


if __name__ == "__main__":
    main()
