from contextlib import closing, contextmanager, redirect_stdout
import fcntl
import io
import json
import os
from pathlib import Path
import subprocess
import sqlite3
import tempfile
import unittest
import uuid
from unittest.mock import patch

from harness_talk import discovery
from harness_talk.cli import main


class Discovery(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.path = Path(temporary.name)
        self.ident = str(uuid.uuid4())

    def claude_row(self, pid=123):
        path = self.path / f"{pid}.sock"
        socket_check = patch.object(discovery, "owned_socket", return_value=str(path))
        socket_check.start()
        self.addCleanup(socket_check.stop)
        row = {"sessionId": self.ident, "cwd": str(self.path), "pid": pid}
        metadata = self.path / f".claude/sessions/{pid}.json"
        metadata.parent.mkdir(parents=True, exist_ok=True)
        metadata.write_text(json.dumps({**row, "messagingSocketPath": str(path)}))
        return row, metadata

    def test_claude_verifies_each_address_without_messages_or_duplicate_selection(self):
        row, _ = self.claude_row()
        mismatch = {**row, "pid": 456}
        # One malformed record cannot hide a separately verified session.
        listed = subprocess.CompletedProcess([], 0, json.dumps([row, {"broken": True}]))
        with patch.object(discovery.Path, "home", return_value=self.path), \
                patch.object(discovery.subprocess, "run", return_value=listed) as run:
            result = discovery.discover_claude()
        self.assertEqual([self.ident], [s["session_id"] for s in result["sessions"]])
        self.assertEqual("running", result["sessions"][0]["runtime_status"])
        self.assertEqual("partial", result["sources"][0]["status"])
        run.assert_called_once_with(["claude", "agents", "--json"], capture_output=True,
                                    text=True, check=True, timeout=15)
        listed.stdout = json.dumps([row, mismatch])
        with patch.object(discovery.Path, "home", return_value=self.path), \
                patch.object(discovery.subprocess, "run", return_value=listed):
            result = discovery.discover_claude()
        self.assertEqual([], result["sessions"])
        self.assertEqual(2, result["sources"][0]["rejected"])

    def test_claude_changed_identity_and_missing_cli_are_not_empty_success(self):
        row, metadata = self.claude_row()
        metadata.write_text(json.dumps({**row, "cwd": "/other"}))
        with patch.object(discovery.Path, "home", return_value=self.path), \
                patch.object(discovery.subprocess, "run", return_value=subprocess.CompletedProcess(
                    [], 0, json.dumps([row]))):
            result = discovery.discover_claude()
        self.assertEqual([], result["sessions"])
        self.assertEqual("partial", result["sources"][0]["status"])
        with patch.object(discovery.subprocess, "run", side_effect=FileNotFoundError):
            result = discovery.discover_claude()
        self.assertEqual("unavailable", result["sources"][0]["status"])

    def test_codex_pages_loaded_ids_and_never_reads_turns_or_resumes(self):
        gone, other = str(uuid.uuid4()), str(uuid.uuid4())
        calls = []
        class Rpc:
            def call(inner, method, params):
                calls.append((method, params))
                if method == "thread/loaded/list":
                    return ({"data": [self.ident, gone], "nextCursor": "next"}
                            if params["cursor"] is None else {"data": [other], "nextCursor": None})
                if method == "thread/read":
                    ident = params["threadId"]
                    return {"thread": {"id": ident, "cwd": str(self.path),
                                       "status": {"type": "notLoaded" if ident == gone else "idle"}}}
                raise AssertionError(method)
        @contextmanager
        def connect(peer):
            self.assertEqual(str(self.path / "codex.sock"), peer["socket"])
            yield Rpc()
        with patch.object(discovery, "codex_rpc", connect):
            result = discovery.discover_codex([self.path / "codex.sock"])
        self.assertEqual([self.ident, other], [s["session_id"] for s in result["sessions"]])
        self.assertEqual("ok", result["sources"][0]["status"])
        self.assertEqual(2, sum(method == "thread/loaded/list" for method, _ in calls))
        self.assertTrue(all(params["includeTurns"] is False for method, params in calls if method == "thread/read"))

    def test_codex_partial_results_survive_later_transport_failure(self):
        class Rpc:
            def call(inner, method, params):
                if method == "thread/loaded/list":
                    if params["cursor"]: raise OSError("connection_closed")
                    return {"data": [self.ident], "nextCursor": "next"}
                return {"thread": {"id": self.ident, "cwd": str(self.path), "status": {"type": "active"}}}
        @contextmanager
        def connect(peer):
            yield Rpc()
        with patch.object(discovery, "codex_rpc", connect):
            result = discovery.discover_codex([self.path / "codex.sock"])
        self.assertEqual(1, len(result["sessions"]))
        self.assertEqual("partial", result["sources"][0]["status"])
        with patch.dict(os.environ, {"CODEX_HOME": str(self.path)}):
            self.assertEqual(self.path / "app-server-control/app-server-control.sock", discovery.default_codex_socket())
            result = discovery.discover_codex()
        self.assertEqual([], result["sessions"])
        self.assertEqual("unavailable", result["sources"][0]["status"])

    @unittest.skipUnless(Path("/proc/locks").exists(), "Linux kernel lock metadata")
    def test_native_codex_requires_a_held_writer_and_saved_cli_address(self):
        directory = self.path / "thread-writer-locks"
        directory.mkdir()
        stale, worker = str(uuid.uuid4()), str(uuid.uuid4())
        (directory / f"{stale}.lock").touch()
        lease = (directory / f"{self.ident}.lock").open("w")
        self.addCleanup(lease.close)
        fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
        worker_lease = (directory / f"{worker}.lock").open("w")
        self.addCleanup(worker_lease.close)
        fcntl.flock(worker_lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
        with closing(sqlite3.connect(self.path / "state_5.sqlite")) as db, db:
            db.execute("CREATE TABLE threads (id TEXT, cwd TEXT, archived INTEGER, source TEXT)")
            db.executemany("INSERT INTO threads VALUES (?, ?, 0, ?)", [
                (self.ident, str(self.path), "cli"), (stale, str(self.path), "cli"),
                (worker, str(self.path), "subAgent")])
        with patch.dict(os.environ, {"CODEX_HOME": str(self.path), "CODEX_SQLITE_HOME": str(self.path)}):
            result = discovery.discover_codex_writers()
            self.assertEqual([self.ident], [row["session_id"] for row in result["sessions"]])
            self.assertEqual("writer_active", result["sessions"][0]["runtime_status"])
            self.assertEqual(os.getpid(), result["sessions"][0]["pid"])
            fcntl.flock(lease, fcntl.LOCK_UN)
            self.assertEqual([], discovery.discover_codex_writers()["sessions"])
        self.assertTrue((directory / f"{self.ident}.lock").exists())

    def test_native_codex_failure_does_not_hide_available_socket_results(self):
        candidate = {"harness": "codex", "session_id": self.ident, "workspace": str(self.path), "runtime_status": "idle"}
        socket_result = {"sessions": [candidate], "sources": [{"harness": "codex", "status": "ok"}]}
        with patch.object(discovery, "discover_codex", return_value=socket_result), \
                patch.dict(os.environ, {"CODEX_HOME": str(self.path)}):
            result = discovery.discover(harness="codex")
        self.assertEqual([candidate], result["sessions"])
        self.assertEqual(["ok", "unavailable"], [source["status"] for source in result["sources"]])

    def test_cli_discovery_filters_without_creating_the_database(self):
        other = {"harness": "codex", "session_id": str(uuid.uuid4()), "workspace": "/other"}
        wanted = {**other, "session_id": self.ident, "workspace": str(self.path), "runtime_status": "idle"}
        output = io.StringIO()
        db = self.path / "absent/mail.sqlite3"
        with patch.object(discovery, "discover_codex", return_value={
                "sessions": [other, wanted], "sources": [{"harness": "codex", "status": "ok"}]}), redirect_stdout(output):
            code = main(["--db", str(db), "peer", "discover", "--harness", "codex", "--workspace", str(self.path)])
        self.assertEqual(0, code)
        self.assertEqual([wanted], json.loads(output.getvalue())["sessions"])
        self.assertFalse(db.parent.exists())

    def test_cli_unavailable_source_has_diagnostics_and_nonzero_exit(self):
        output = io.StringIO()
        with patch.object(discovery.subprocess, "run", side_effect=FileNotFoundError), redirect_stdout(output):
            code = main(["peer", "discover", "--harness", "claude"])
        self.assertEqual(2, code)
        result = json.loads(output.getvalue())
        self.assertEqual("unavailable", result["sources"][0]["status"])
        self.assertIn("do not prove", result["scope"])


if __name__ == "__main__":
    unittest.main()
