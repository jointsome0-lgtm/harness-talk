import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import uuid
from unittest.mock import patch

from harness_talk import adapters, opencode
from harness_talk.cli import main
from harness_talk.errors import CodedOSError, CodedValueError, failure_detail
from harness_talk.store import Store


class FailureDetails(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.path = Path(temporary.name)
        self.store = Store(self.path / "mail.sqlite3")
        for name in ("alice", "bob"):
            self.store.add_peer(name, "claude", str(uuid.uuid4()), self.path)

    def test_only_htalk_codes_are_recorded_verbatim(self):
        self.assertEqual("recipient_unavailable", failure_detail(CodedValueError("recipient_unavailable")))
        self.assertEqual("opencode_RemoteDisconnected", failure_detail(opencode.Uncertain("opencode_RemoteDisconnected")))
        # Foreign text is dropped even when it looks like a code or names a path.
        self.assertEqual("ValueError", failure_detail(ValueError("secret_token")))
        self.assertEqual("PermissionError", failure_detail(PermissionError(13, "Permission denied", "/home/user/.private")))

    def test_notification_and_cleanup_details_keep_codes_and_hide_foreign_text(self):
        question = self.store.save("alice", "bob", "Question")[0]
        failed = self.store.notify_once(question["id"], unittest.mock.Mock(side_effect=ValueError("secret_token")))
        self.assertEqual(("submission_unknown", "ValueError"), (failed["submission"], failed["notification_detail"]))
        other = self.store.save("alice", "bob", "Another question")[0]
        closed = self.store.notify_once(other["id"], unittest.mock.Mock(side_effect=CodedOSError("codex_rpc_closed")))
        self.assertEqual("codex_rpc_closed", closed["notification_detail"])
        cleaning = Store(self.store.path, dismiss_notification=unittest.mock.Mock(side_effect=OSError("/private/path")))
        self.assertEqual({"status": "unknown", "detail": "OSError"}, cleaning.ack(question["id"], "bob")["notification_cleanup"])

    def test_unavailable_claude_recipient_reports_its_code(self):
        question = self.store.save("alice", "bob", "Question")[0]
        with patch.object(adapters.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "[]")):
            saved = self.store.notify_once(question["id"], adapters.notify)
        self.assertEqual(("not_submitted", "recipient_unavailable"), (saved["submission"], saved["notification_detail"]))

    def test_codex_rpc_rejection_keeps_only_an_integer_code(self):
        class Connection:
            def __init__(self, error):
                self.error = error
            def send(self, text):
                pass
            def recv(self, timeout):
                return json.dumps({"id": 1, "error": self.error})
        for error, detail in (({"code": -32600, "message": "/private/path"}, "codex_rpc_rejected:-32600"),
                              ({"code": "-32600; secret"}, "codex_rpc_rejected"),
                              ({"code": True}, "codex_rpc_rejected"),
                              ("unstructured", "codex_rpc_rejected")):
            with self.assertRaises(CodedValueError) as raised:
                adapters.Rpc(Connection(error)).call("thread/read", {})
            self.assertEqual(detail, failure_detail(raised.exception))

    def test_send_with_a_saved_answer_exits_zero_despite_unconfirmed_notice(self):
        def answered(peer, message, path):
            Store(path).save("bob", "alice", "Answer", in_reply_to=message["id"])
            return "not_submitted", "recipient_unavailable"
        with patch.dict(os.environ, {}, clear=True), patch("builtins.print") as output:
            with patch("harness_talk.cli.notify", side_effect=answered):
                self.assertEqual(0, main(["--db", str(self.store.path), "--as", "alice", "send", "bob",
                                         "--message", "Question", "--wait", "1"]))
            result = json.loads(output.call_args.args[0])
            self.assertEqual(("not_submitted", "Answer"), (result["submission"], result["reply"]["body"]))
            with patch("harness_talk.cli.notify", return_value=("not_submitted", "recipient_unavailable")):
                self.assertEqual(2, main(["--db", str(self.store.path), "--as", "alice", "send", "bob",
                                         "--message", "Unanswered question"]))


if __name__ == "__main__":
    unittest.main()
