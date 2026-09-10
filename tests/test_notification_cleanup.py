from concurrent.futures import ThreadPoolExecutor
from contextlib import closing
import json
import os
from pathlib import Path
import shlex
import sqlite3
import subprocess
import sys
import tempfile
import threading
import unittest
import uuid
from unittest.mock import Mock, patch

from harness_talk import adapters
from harness_talk.cli import main
from harness_talk.store import Store


class NotificationCleanup(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.path = Path(temporary.name)
        self.queue = set()
        self.cleanup = Mock(side_effect=self.dismiss)
        self.store = Store(self.path / "mail.sqlite3", dismiss_notification=self.cleanup)
        self.store.add_peer("sender", "claude", str(uuid.uuid4()), self.path)
        self.store.add_peer("reader", "codex", str(uuid.uuid4()), self.path)
        self.message, _ = self.store.save("sender", "reader", "Read this question")
        self.queue_id = str(uuid.uuid4())

    def submit(self, *args):
        self.queue.add(self.queue_id)
        return "submitted", "codex_cli_queued:" + self.queue_id

    def dismiss(self, peer, message, path):
        self.assertEqual("reader", peer["name"])
        self.assertIsNotNone(Store(path).get(message["id"])["ack_at"])
        if message["notification_detail"] is None:
            return {"status": "skipped"}
        queue_id = message["notification_detail"].split(":", 1)[1]
        removed = queue_id in self.queue
        self.queue.discard(queue_id)
        return {"status": "removed" if removed else "absent", "queue_id": queue_id}

    def test_cli_read_keeps_notice_and_ack_removes_only_its_notice(self):
        self.store.notify_once(self.message["id"], self.submit)
        self.queue.add("unrelated-user-input")
        args = ["--db", str(self.store.path), "--as", "reader"]
        with patch.dict(os.environ, {}, clear=True), patch("builtins.print") as output, \
                patch("harness_talk.cli.dismiss_notification", self.cleanup):
            self.assertEqual(0, main([*args, "inbox"]))
            self.assertEqual(0, main([*args, "show", self.message["id"]]))
            self.cleanup.assert_not_called()
            self.assertEqual(0, main([*args, "ack", self.message["id"]]))
            ack = json.loads(output.call_args.args[0])
            self.assertEqual("removed", ack["notification_cleanup"]["status"])
            self.assertEqual({"unrelated-user-input"}, self.queue)
            self.assertEqual(0, main([*args, "ack", self.message["id"]]))
            again = json.loads(output.call_args.args[0])
            self.assertEqual("absent", again["notification_cleanup"]["status"])
            self.assertEqual(ack["ack_at"], again["ack_at"])
        saved = self.store.get(self.message["id"])
        self.assertEqual("submitted", saved["submission"])
        self.assertEqual("codex_cli_queued:" + self.queue_id, saved["notification_detail"])
        self.assertEqual([self.message["id"]], [m["id"] for m in self.store.inbox("reader")["messages"]])

    def test_ack_before_notification_prevents_submission(self):
        self.store.ack(self.message["id"], "reader")
        notify = Mock(side_effect=self.submit)
        result = self.store.notify_once(self.message["id"], notify)
        notify.assert_not_called()
        self.assertIsNone(result["notification_started_at"])
        self.assertEqual(set(), self.queue)

    def test_ack_during_submission_is_cleaned_up_when_receipt_arrives(self):
        submitting, finish = threading.Event(), threading.Event()

        def delayed(*args):
            submitting.set()
            if not finish.wait(5):
                raise TimeoutError("test did not release submission")
            return self.submit(*args)

        with ThreadPoolExecutor(max_workers=1) as executor:
            pending = executor.submit(self.store.notify_once, self.message["id"], delayed)
            try:
                self.assertTrue(submitting.wait(5))
                reader = Store(self.store.path, dismiss_notification=self.cleanup)
                ack = reader.ack(self.message["id"], "reader")
                self.assertEqual("skipped", ack["notification_cleanup"]["status"])
            finally:
                finish.set()
            result = pending.result(timeout=5)
        self.assertEqual("removed", result["notification_cleanup"]["status"])
        self.assertEqual(set(), self.queue)
        self.assertIsNotNone(result["ack_at"])

    def test_cleanup_failure_preserves_ack_and_can_be_retried_without_notifying(self):
        self.store.notify_once(self.message["id"], self.submit)
        self.cleanup.side_effect = TimeoutError()
        first = self.store.ack(self.message["id"], "reader")
        self.assertEqual("unknown", first["notification_cleanup"]["status"])
        self.assertEqual(first["ack_at"], Store(self.store.path).get(self.message["id"])["ack_at"])
        self.cleanup.side_effect = self.dismiss
        second = self.store.ack(self.message["id"], "reader")
        self.assertEqual("removed", second["notification_cleanup"]["status"])
        self.assertEqual(first["ack_at"], second["ack_at"])
        self.cleanup.reset_mock()
        with self.assertRaisesRegex(ValueError, "only_recipient"):
            self.store.ack(self.message["id"], "sender")
        self.cleanup.assert_not_called()


class CodexCleanup(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.path = Path(temporary.name)
        environment = patch.dict(os.environ, {"CODEX_HOME": str(self.path), "CODEX_SQLITE_HOME": ""})
        environment.start()
        self.addCleanup(environment.stop)
        self.peer = {"name": "reader", "harness": "codex", "session_id": str(uuid.uuid4()),
                     "workspace": str(self.path), "socket": None}
        self.queue_id = str(uuid.uuid4())
        self.message = {"id": str(uuid.uuid4()), "recipient": "reader", "ack_at": 123,
                        "submission": "submitted", "notification_detail": "codex_cli_queued:" + self.queue_id}
        with closing(sqlite3.connect(self.path / "state_5.sqlite")) as db, db:
            db.execute("CREATE TABLE threads (id TEXT, cwd TEXT, archived INTEGER, source TEXT)")
            db.execute("INSERT INTO threads VALUES (?, ?, 0, 'cli')", (self.peer["session_id"], str(self.path)))

    def test_native_cleanup_deletes_exact_receipt_and_reports_absence_or_uncertainty(self):
        for result, expected in (({"deleted": True}, "removed"), ({"deleted": False}, "absent"),
                                 ({"deleted": "true"}, "unknown"), (TimeoutError(), "unknown")):
            with self.subTest(expected=expected), patch.object(adapters, "codex_stdio_rpc") as connect:
                rpc = connect.return_value.__enter__.return_value
                if isinstance(result, Exception):
                    rpc.call.side_effect = result
                else:
                    rpc.call.return_value = result
                cleanup = adapters.dismiss_notification(self.peer, self.message, self.path / "mail.sqlite3")
                self.assertEqual(expected, cleanup["status"])
                rpc.call.assert_called_once_with("thread/queue/delete", {
                    "threadId": self.peer["session_id"], "queuedSubmissionId": self.queue_id})

    def test_in_flight_answer_keeps_recipient_recovery_after_sender_exits(self):
        sender = Store(self.path / "mail.sqlite3")
        sender.add_peer("reader", "codex", self.peer["session_id"], self.path)
        sender.add_peer("sender", "claude", str(uuid.uuid4()), self.path)
        question, _ = sender.save("reader", "sender", "Question")
        answer, _ = sender.save("sender", "reader", "Answer", in_reply_to=question["id"])
        acknowledgments = []

        def submitting(*args):
            with patch("builtins.print") as output:
                self.assertEqual(0, main(["--db", str(sender.path), "--as", "reader", "ack", answer["id"]]))
                acknowledgments.append(json.loads(output.call_args.args[0]))
            return "submitted", "codex_cli_queued:" + self.queue_id

        # No sender cleanup callback: model a process ending once its receipt is saved.
        with patch.dict(os.environ, {"CODEX_THREAD_ID": self.peer["session_id"]}):
            sender.notify_once(answer["id"], submitting)
            ack = acknowledgments[0]
            self.assertEqual("pending", ack["notification_cleanup"]["status"])
            self.assertEqual([], sender.inbox("reader")["messages"])
            retry = shlex.split(ack["recovery"]["retry_notification_cleanup"])
            with patch.object(adapters, "codex_stdio_rpc") as connect, patch("builtins.print") as output:
                connect.return_value.__enter__.return_value.call.return_value = {"deleted": True}
                self.assertEqual(0, main(retry[1:]))
                result = json.loads(output.call_args.args[0])
            self.assertEqual("removed", result["notification_cleanup"]["status"])
            self.assertEqual(ack["ack_at"], result["ack_at"])

    def test_no_removal_without_ack_confirmed_receipt_and_exact_native_identity(self):
        for change in ({"ack_at": None}, {"recipient": "other"}, {"submission": "submission_unknown"},
                       {"notification_detail": "codex_cli_queued:invalid"}, {"notification_detail": None}):
            with self.subTest(change=change), patch.object(adapters, "codex_stdio_rpc") as connect:
                adapters.dismiss_notification(self.peer, {**self.message, **change}, self.path / "mail.sqlite3")
                connect.assert_not_called()
        with closing(sqlite3.connect(self.path / "state_5.sqlite")) as db, db:
            db.execute("UPDATE threads SET cwd='/other-workspace'")
        with patch.object(adapters, "codex_stdio_rpc") as connect:
            result = adapters.dismiss_notification(self.peer, self.message, self.path / "mail.sqlite3")
            self.assertEqual("unavailable", result["status"])
            connect.assert_not_called()

    def test_explicit_socket_checks_identity_and_never_falls_back_to_native(self):
        self.peer["socket"] = str(self.path / "codex.sock")
        self.message["notification_detail"] = "codex_queued:" + self.queue_id
        with patch.object(adapters, "codex_rpc") as connect, \
                patch.object(adapters, "check_codex") as check, \
                patch.object(adapters, "codex_stdio_rpc") as native:
            rpc = connect.return_value.__enter__.return_value
            rpc.call.return_value = {"deleted": True}
            self.assertEqual("removed", adapters.dismiss_notification(self.peer, self.message, self.path)["status"])
            check.assert_called_once_with(rpc, self.peer)
            check.side_effect = ValueError("recipient_identity_changed")
            rpc.reset_mock()
            self.assertEqual("unavailable", adapters.dismiss_notification(self.peer, self.message, self.path)["status"])
            rpc.call.assert_not_called()
            native.assert_not_called()

    def test_stdio_protocol_with_real_process_handles_buffered_notifications_and_exits(self):
        fixture = self.path / "fake_codex.py"
        fixture.write_text('''import json, sys
for raw in sys.stdin:
    frame = json.loads(raw)
    if frame['method'] == 'initialized':
        continue
    assert frame['method'] in ('initialize', 'thread/queue/delete')
    result = {} if frame['method'] == 'initialize' else {'deleted': True}
    notice = json.dumps({'method': 'irrelevant/notification'}) + '\\n'
    reply = json.dumps({'id': frame['id'], 'result': result}) + '\\n'
    sys.stdout.write(notice + reply)
    sys.stdout.flush()
''')
        popen = subprocess.Popen
        processes = []

        def start(args, **kwargs):
            self.assertEqual(["codex", "app-server", "--stdio"], args)
            process = popen([sys.executable, str(fixture)], **kwargs)
            processes.append(process)
            return process

        with patch.object(adapters.subprocess, "Popen", side_effect=start):
            result = adapters.dismiss_notification(self.peer, self.message, self.path / "mail.sqlite3")
        self.assertEqual("removed", result["status"])
        self.assertEqual(0, processes[0].returncode)


if __name__ == "__main__":
    unittest.main()
