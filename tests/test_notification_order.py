"""An answer returned by its recipient's wait carries a receipt; a claimed notice for
it is skipped in the store and at every transport's final check. The receipt is not an
acknowledgment. A registered wait is only a hint to wait briefly for that receipt."""
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
import time
import unittest
import uuid
from unittest.mock import Mock, patch

from harness_talk import adapters, store as storage
from harness_talk.cli import main
from harness_talk.store import Store


class WaitReturnReceipt(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.path = Path(temporary.name)
        self.store = Store(self.path / "mail.sqlite3")
        self.store.add_peer("builder", "claude", str(uuid.uuid4()), self.path)
        self.store.add_peer("reviewer", "claude", str(uuid.uuid4()), self.path)
        self.request, _ = self.store.save("builder", "reviewer", "Which case needs another test?")
        self.notify = Mock(return_value=("submitted", "claude_socket_bytes_written"))

    def answer(self, body="Answer"):
        return self.store.save("reviewer", "builder", body, in_reply_to=self.request["id"])[0]

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

    def assert_skipped(self, result):
        self.notify.assert_not_called()
        self.assertEqual(("not_submitted", "returned_by_recipient_wait"),
                         (result["submission"], result["notification_detail"]))
        self.assertIsNotNone(result["notification_finished_at"])
        self.assertIsNotNone(result["wait_returned_at"])
        self.assertIsNone(result["ack_at"])
        self.assertEqual([result["id"]], [m["id"] for m in self.store.inbox("builder")["messages"]])

    def test_wait_that_returned_the_answer_before_notify_once_suppresses_the_notice(self):
        # Schedule 1: save, wait observes and returns, registration is gone, then notify_once.
        with ThreadPoolExecutor(max_workers=1) as executor:
            waiting = executor.submit(self.store.wait, self.request["id"], "builder", 10)
            self.assertTrue(self.wait_registered())
            answer = self.answer()
            received = waiting.result(timeout=5)
        self.assertEqual(answer["id"], received["reply"]["id"])
        self.assertIsNotNone(received["reply"]["wait_returned_at"])
        self.assertEqual([], self.waits())
        self.assert_skipped(Store(self.store.path).notify_once(answer["id"], self.notify))
        self.store.notify_once(answer["id"], self.notify)  # The one attempt stays spent.
        self.notify.assert_not_called()

    def test_wait_completing_during_adapter_preflight_is_seen_by_the_final_check(self):
        # Schedule 2: no registration when notify_once starts; the wait returns the answer
        # while the client preflight runs, before the frame is written.
        answer = self.answer()
        received = {}
        with patch.object(adapters, "claude_socket", side_effect=lambda peer: (
                received.update(Store(self.store.path).wait(self.request["id"], "builder", 5)), "/socket")[1]), \
                patch.object(adapters.socket, "socket") as socket:
            connection = socket.return_value.__enter__.return_value
            result = self.store.notify_once(answer["id"], adapters.notify)
            connection.sendall.assert_not_called()
        self.assertEqual(answer["id"], received["reply"]["id"])
        self.assertEqual(("not_submitted", "returned_by_recipient_wait"),
                         (result["submission"], result["notification_detail"]))
        self.assertIsNone(result["ack_at"])

    def test_reply_saved_during_a_running_wait_gives_the_poll_time_to_return_it(self):
        with ThreadPoolExecutor(max_workers=1) as executor:
            waiting = executor.submit(self.store.wait, self.request["id"], "builder", 10)
            self.assertTrue(self.wait_registered())
            answer = self.answer()
            started = time.monotonic()
            result = Store(self.store.path).notify_once(answer["id"], self.notify)
            self.assertLess(time.monotonic() - started, storage.WAIT_GRACE)
            received = waiting.result(timeout=5)
        self.assert_skipped(result)
        self.assertEqual(answer["id"], received["reply"]["id"])

    def test_without_a_wait_or_after_a_passive_read_the_answer_is_notified(self):
        answer = self.answer()
        self.store.get(answer["id"], "builder")
        self.store.inbox("builder")
        self.store.sent("reviewer")
        self.assertIsNone(self.store.get(answer["id"])["wait_returned_at"])
        self.assertEqual("submitted", self.store.notify_once(answer["id"], self.notify)["submission"])
        self.notify.assert_called_once()
        self.assertIsNone(self.store.skip_reason(self.request))
        second, _ = self.store.save("builder", "reviewer", "Second question")
        self.assertEqual("timeout", self.store.wait(second["id"], "builder", 0.2)["wait_ended"])
        self.assertEqual([], self.waits())

    def test_killed_waiter_only_delays_the_notice_by_the_grace_bound(self):
        process = subprocess.Popen([sys.executable, "-m", "harness_talk.cli", "--db", str(self.store.path),
                                    "--as", "builder", "wait", self.request["id"], "--seconds", "45"],
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            self.assertTrue(self.wait_registered())
        finally:
            process.send_signal(signal.SIGKILL)
            process.wait(timeout=5)
        self.assertEqual(1, len(self.waits()))  # The stale registration outlives the process.
        answer = self.answer()
        started = time.monotonic()
        result = self.store.notify_once(answer["id"], self.notify)
        elapsed = time.monotonic() - started
        self.notify.assert_called_once()
        self.assertEqual("submitted", result["submission"])
        self.assertIsNone(result["wait_returned_at"])
        self.assertGreaterEqual(elapsed, storage.WAIT_GRACE * 0.9)
        self.assertLess(elapsed, storage.WAIT_GRACE + 1)

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
        self.assertEqual(0, process.returncode, stderr)
        self.assert_skipped(self.store.get(answer["id"]))
        self.assertIn("no client notice was sent", answer["next_action"])
        received = json.loads(stdout)
        self.assertEqual(answer["id"], received["reply"]["id"])
        self.assertIsNotNone(received["reply"]["wait_returned_at"])
        self.assertIn("ack_after_reading", received["recovery"])
        self.assertEqual([], self.waits())

    def test_interrupted_or_ended_wait_forgets_its_registration_and_records_nothing(self):
        with patch.object(storage.time, "sleep", side_effect=KeyboardInterrupt()):
            with self.assertRaises(KeyboardInterrupt):
                self.store.wait(self.request["id"], "builder", 5)
        self.assertEqual([], self.waits())
        self.assertEqual("timeout", self.store.wait(self.request["id"], "builder", 0)["wait_ended"])
        self.assertEqual([], self.waits())
        answer = self.answer()
        self.assertIsNone(self.store.get(answer["id"])["wait_returned_at"])
        # A single check that returns the answer still records the receipt.
        self.assertEqual(answer["id"], self.store.wait(self.request["id"], "builder", 0)["reply"]["id"])
        self.assertEqual("returned_by_recipient_wait", self.store.skip_reason(answer))
        self.store.ack(answer["id"], "builder")
        self.assertEqual("acknowledged_before_notification", self.store.skip_reason(answer))

    def test_existing_database_gains_the_receipt_column_and_table_without_a_version_change(self):
        with closing(sqlite3.connect(self.store.path)) as db, db:
            db.execute("DROP TABLE waits")
            db.execute("ALTER TABLE messages DROP COLUMN wait_returned_at")
        reopened = Store(self.store.path)
        with closing(sqlite3.connect(reopened.path)) as db:
            self.assertEqual(storage.SCHEMA_VERSION, db.execute("PRAGMA user_version").fetchone()[0])
            self.assertEqual([], db.execute("SELECT * FROM waits").fetchall())
        self.assertIsNone(reopened.get(self.request["id"])["wait_returned_at"])


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

    def returned_by_wait(self, request, actor):
        self.assertIsNotNone(Store(self.store.path).wait(request["id"], actor, 0)["reply"])

    def test_claude_frame_is_not_written_for_an_answer_the_recipient_wait_returned(self):
        peer, request, answer = self.exchange("claude")
        with patch.object(adapters, "claude_socket", return_value="/socket"), patch.object(adapters.socket, "socket") as socket:
            connection = socket.return_value.__enter__.return_value
            connection.connect.side_effect = lambda *_: self.returned_by_wait(request, peer["name"])
            result = self.store.notify_once(answer["id"], adapters.notify)
            connection.sendall.assert_not_called()
        self.assertEqual(("not_submitted", "returned_by_recipient_wait"),
                         (result["submission"], result["notification_detail"]))

    def test_codex_queue_command_is_not_run_after_a_late_acknowledgment_or_wait_return(self):
        peer, request, answer = self.exchange("codex")
        identity = adapters.codex_saved_identity
        for late in (lambda: self.returned_by_wait(request, peer["name"]),
                     lambda: Store(self.store.path).ack(answer["id"], peer["name"])):
            with patch.object(adapters, "codex_saved_identity", side_effect=lambda p: (identity(p), late())[0]), \
                    patch.object(adapters.subprocess, "run") as run:
                submission, detail = adapters.notify(peer, answer, self.store.path)
                run.assert_not_called()
            self.assertEqual("not_submitted", submission)
            self.assertIn(detail, storage.NOT_NEEDED)
        # Without those receipts the ordinary queue attempt is made.
        with closing(sqlite3.connect(self.store.path)) as db, db:
            db.execute("UPDATE messages SET ack_at=NULL, wait_returned_at=NULL")
        with patch.object(adapters.subprocess, "run") as run:
            run.return_value = subprocess.CompletedProcess([], 0, f"Queued message {uuid.uuid4()} for thread {peer['session_id']}.\n", "")
            self.assertEqual("submitted", adapters.notify(peer, answer, self.store.path)[0])

    def test_codex_socket_queue_add_is_not_called_after_a_late_wait_return(self):
        peer, request, answer = self.exchange("codex", self.path / "codex.sock")
        rpc = Mock()
        with patch.object(adapters, "codex_rpc") as connect, \
                patch.object(adapters, "check_codex", side_effect=lambda *_: self.returned_by_wait(request, peer["name"])):
            connect.return_value.__enter__.return_value = rpc
            submission, detail = adapters.notify(peer, answer, self.store.path)
        rpc.call.assert_not_called()
        self.assertEqual(("not_submitted", "returned_by_recipient_wait"), (submission, detail))


if __name__ == "__main__":
    unittest.main()
