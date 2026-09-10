"""Answers a requester is polling for are read by that poll; their client notice
is skipped. Every transport rereads the saved state immediately before its write."""
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing
import json
import os
from pathlib import Path
import signal
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import uuid
from unittest.mock import Mock, patch

from harness_talk import adapters, store as storage
from harness_talk.cli import main
from harness_talk.store import Store


class ActiveWait(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.path = Path(temporary.name)
        self.store = Store(self.path / "mail.sqlite3")
        self.store.add_peer("builder", "claude", str(uuid.uuid4()), self.path)
        self.store.add_peer("reviewer", "claude", str(uuid.uuid4()), self.path)
        self.request, _ = self.store.save("builder", "reviewer", "Which case needs another test?")
        self.notify = Mock(return_value=("submitted", "claude_socket_bytes_written"))

    def waits(self):
        with closing(sqlite3.connect(self.store.path)) as db:
            return db.execute("SELECT message_id, actor, until FROM waits").fetchall()

    def wait_registered(self, timeout=5):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.waits():
                return True
            time.sleep(0.02)
        return False

    def test_answer_during_active_wait_is_read_by_the_poll_and_not_notified(self):
        with ThreadPoolExecutor(max_workers=1) as executor:
            waiting = executor.submit(self.store.wait, self.request["id"], "builder", 10)
            self.assertTrue(self.wait_registered())
            answer, _ = self.store.save("reviewer", "builder", "Test recovery after interruption.",
                                        in_reply_to=self.request["id"])
            result = Store(self.store.path).notify_once(answer["id"], self.notify)
            received = waiting.result(timeout=5)
        self.notify.assert_not_called()
        self.assertEqual(("not_submitted", "recipient_waiting_for_this_answer"),
                         (result["submission"], result["notification_detail"]))
        self.assertIsNotNone(result["notification_finished_at"])
        self.assertEqual(answer["id"], received["reply"]["id"])
        self.assertIsNone(received["reply"]["ack_at"])  # Reading still acknowledges nothing.
        self.assertEqual([], self.waits())
        # The one attempt is spent: a later ack or wait never notifies either.
        self.store.notify_once(answer["id"], self.notify)
        self.notify.assert_not_called()
        # Without an active poll, the same answer is notified as before.
        later, _ = self.store.save("builder", "reviewer", "Second question")
        reply, _ = self.store.save("reviewer", "builder", "Second answer", in_reply_to=later["id"])
        self.assertEqual("submitted", self.store.notify_once(reply["id"], self.notify)["submission"])
        self.notify.assert_called_once()

    def test_only_a_fresh_wait_by_the_recipient_on_this_request_skips_the_notice(self):
        answer, _ = self.store.save("reviewer", "builder", "Answer", in_reply_to=self.request["id"])
        other, _ = self.store.save("builder", "reviewer", "Other question")
        now = time.time()
        for label, row in (("expired", (self.request["id"], "builder", now - 10)),
                           ("about to time out", (self.request["id"], "builder", now + storage.WAIT_MARGIN / 2)),
                           ("other actor", (self.request["id"], "reviewer", now + 30)),
                           ("other request", (other["id"], "builder", now + 30))):
            with self.subTest(label=label):
                self.store.register_wait(str(uuid.uuid4()), *row)
                self.assertIsNone(self.store.skip_reason(answer))
        self.store.register_wait(str(uuid.uuid4()), self.request["id"], "builder", now + 30)
        self.assertEqual("recipient_waiting_for_this_answer", self.store.skip_reason(answer))
        self.store.ack(answer["id"], "builder")
        self.assertEqual("acknowledged_before_notification", self.store.skip_reason(answer))
        self.assertIsNone(self.store.skip_reason(self.request))  # A request has no poll to satisfy it.

    def test_killed_waiter_process_expires_within_the_heartbeat_bound(self):
        process = subprocess.Popen([sys.executable, "-m", "harness_talk.cli", "--db", str(self.store.path),
                                    "--as", "builder", "wait", self.request["id"], "--seconds", "45"],
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            self.assertTrue(self.wait_registered())
            time.sleep(storage.WAIT_HEARTBEAT * 1.5)  # At least one refresh has happened.
            [(message_id, actor, until)] = self.waits()
            self.assertEqual((self.request["id"], "builder"), (message_id, actor))
            self.assertLessEqual(until - time.time(), storage.WAIT_TTL)
        finally:
            process.send_signal(signal.SIGKILL)
            process.wait(timeout=5)
        # The registration outlives the process only until its last refresh expires.
        answer, _ = self.store.save("reviewer", "builder", "Answer", in_reply_to=self.request["id"])
        self.assertEqual(1, len(self.waits()))
        deadline = time.monotonic() + storage.WAIT_TTL + 1
        while self.store.skip_reason(answer) is not None:
            self.assertLess(time.monotonic(), deadline, "killed waiter did not expire")
            time.sleep(0.05)
        self.assertEqual("submitted", self.store.notify_once(answer["id"], self.notify)["submission"])

    def test_cli_reply_to_a_waiting_process_exits_zero_and_the_wait_returns_the_answer(self):
        process = subprocess.Popen([sys.executable, "-m", "harness_talk.cli", "--db", str(self.store.path),
                                    "--as", "builder", "wait", self.request["id"], "--seconds", "20"],
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            self.assertTrue(self.wait_registered())
            args = ["--db", str(self.store.path), "--as", "reviewer"]
            with patch.dict(os.environ, {}, clear=True), patch("builtins.print") as output, \
                    patch("harness_talk.cli.notify", self.notify):
                self.assertEqual(0, main([*args, "reply", self.request["id"], "--message", "Answer"]))
                answer = json.loads(output.call_args.args[0])
            stdout, stderr = process.communicate(timeout=10)
        finally:
            if process.poll() is None:
                process.kill()
        self.notify.assert_not_called()
        self.assertEqual(0, process.returncode, stderr)
        self.assertEqual(("not_submitted", "recipient_waiting_for_this_answer"),
                         (answer["submission"], answer["notification_detail"]))
        self.assertIn("no client notice was sent", answer["next_action"])
        received = json.loads(stdout)
        self.assertEqual(answer["id"], received["reply"]["id"])
        self.assertIn("ack_after_reading", received["recovery"])
        self.assertEqual([], self.waits())
        self.assertEqual([answer["id"]], [m["id"] for m in self.store.inbox("builder")["messages"]])

    def test_interrupted_or_ended_wait_forgets_its_registration(self):
        with patch.object(storage.time, "sleep", side_effect=KeyboardInterrupt()):
            with self.assertRaises(KeyboardInterrupt):
                self.store.wait(self.request["id"], "builder", 5)
        self.assertEqual([], self.waits())
        self.assertEqual("timeout", self.store.wait(self.request["id"], "builder", 0.2)["wait_ended"])
        self.assertEqual([], self.waits())
        self.store.wait(self.request["id"], "builder", 0)  # A single check registers nothing.
        self.assertEqual([], self.waits())

    def test_existing_database_gains_the_waits_table_without_a_version_change(self):
        with closing(sqlite3.connect(self.store.path)) as db, db:
            db.execute("DROP TABLE waits")
        reopened = Store(self.store.path)
        with closing(sqlite3.connect(reopened.path)) as db:
            self.assertEqual(storage.SCHEMA_VERSION, db.execute("PRAGMA user_version").fetchone()[0])
            self.assertEqual([], db.execute("SELECT * FROM waits").fetchall())
        self.assertEqual(self.request["id"], reopened.get(self.request["id"])["id"])


class FinalCheckBeforeEachWrite(unittest.TestCase):
    """The reread happens after preflight and immediately before the transport write."""
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.path = Path(temporary.name)
        environment = patch.dict(os.environ, {"CODEX_HOME": str(self.path), "CODEX_SQLITE_HOME": ""})
        environment.start()
        self.addCleanup(environment.stop)
        self.store = Store(self.path / "mail.sqlite3")
        self.store.add_peer("builder", "claude", str(uuid.uuid4()), self.path)

    def exchange(self, harness, socket=None):
        session_id = str(uuid.uuid4())
        peer = self.store.add_peer("reviewer-" + harness, harness, session_id, self.path, socket)
        if harness == "codex":
            with closing(sqlite3.connect(self.path / "state_5.sqlite")) as db, db:
                db.execute("CREATE TABLE IF NOT EXISTS threads (id TEXT PRIMARY KEY, cwd TEXT, archived INTEGER, source TEXT)")
                db.execute("INSERT INTO threads VALUES (?, ?, 0, 'cli')", (session_id, str(self.path)))
        request, _ = self.store.save(peer["name"], "builder", "Question")
        answer, _ = self.store.save("builder", peer["name"], "Answer", in_reply_to=request["id"])
        return peer, request, answer

    def start_wait(self, request, actor):
        self.store.register_wait(str(uuid.uuid4()), request["id"], actor, time.time() + 30)

    def test_claude_frame_is_not_written_for_an_answer_its_recipient_is_polling(self):
        peer, request, answer = self.exchange("claude")
        with patch.object(adapters, "claude_socket", return_value="/socket"), patch.object(adapters.socket, "socket") as socket:
            connection = socket.return_value.__enter__.return_value
            connection.connect.side_effect = lambda *_: self.start_wait(request, peer["name"])
            result = self.store.notify_once(answer["id"], adapters.notify)
            connection.sendall.assert_not_called()
        self.assertEqual(("not_submitted", "recipient_waiting_for_this_answer"),
                         (result["submission"], result["notification_detail"]))

    def test_codex_queue_command_is_not_run_after_a_late_acknowledgment_or_poll(self):
        peer, request, answer = self.exchange("codex")
        identity = adapters.codex_saved_identity
        for late in (lambda: self.start_wait(request, peer["name"]),
                     lambda: Store(self.store.path).ack(answer["id"], peer["name"])):
            with patch.object(adapters, "codex_saved_identity", side_effect=lambda p: (identity(p), late())[0]), \
                    patch.object(adapters.subprocess, "run") as run:
                submission, detail = adapters.notify(peer, answer, self.store.path)
                run.assert_not_called()
            self.assertEqual("not_submitted", submission)
            self.assertIn(detail, storage.NOT_NEEDED)
        # Removing the race restores the ordinary queue attempt.
        with closing(sqlite3.connect(self.store.path)) as db, db:
            db.execute("DELETE FROM waits")
            db.execute("UPDATE messages SET ack_at=NULL")
        with patch.object(adapters.subprocess, "run") as run:
            run.return_value = subprocess.CompletedProcess([], 0, f"Queued message {uuid.uuid4()} for thread {peer['session_id']}.\n", "")
            self.assertEqual("submitted", adapters.notify(peer, answer, self.store.path)[0])

    def test_codex_socket_queue_add_is_not_called_after_a_late_poll(self):
        peer, request, answer = self.exchange("codex", self.path / "codex.sock")
        rpc = Mock()
        with patch.object(adapters, "codex_rpc") as connect, \
                patch.object(adapters, "check_codex", side_effect=lambda *_: self.start_wait(request, peer["name"])):
            connect.return_value.__enter__.return_value = rpc
            submission, detail = adapters.notify(peer, answer, self.store.path)
        rpc.call.assert_not_called()
        self.assertEqual(("not_submitted", "recipient_waiting_for_this_answer"), (submission, detail))


if __name__ == "__main__":
    unittest.main()
