from contextlib import closing
import json
import os
from pathlib import Path
import sqlite3
import tempfile
import time
import unittest
import uuid
from unittest.mock import Mock, patch

from harness_talk import adapters
from harness_talk.cli import main
from harness_talk.store import Store


class RetiredPeers(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.path = Path(temporary.name)
        self.store = Store(self.path / "mail.sqlite3")
        for name in ("alice", "bob"):
            self.store.add_peer(name, "claude", str(uuid.uuid4()), self.path)
        self.notify = Mock(return_value=("submitted", "claude_socket_bytes_written"))

    def cli(self, *words, code=0):
        with patch.dict(os.environ, {}, clear=True), patch("builtins.print") as output, \
                patch("harness_talk.cli.notify", self.notify):
            self.assertEqual(code, main(["--db", str(self.store.path), *words]))
        return json.loads(output.call_args.args[0])

    def test_retired_peers_neither_send_nor_receive_new_requests(self):
        request_id = str(uuid.uuid4())
        saved = self.cli("--as", "alice", "send", "bob", "--message", "Before", "--id", request_id)
        self.assertIsNotNone(self.cli("peer", "retire", "bob")["retired_at"])
        for sender, recipient in (("alice", "bob"), ("bob", "alice")):
            refused = self.cli("--as", sender, "send", recipient, "--message", "After", code=2)
            self.assertEqual(("peer_retired", ["bob"]), (refused["error"], refused["retired_peers"]))
            self.assertIn("Nothing was saved", refused["next_action"])
        self.assertEqual((1, 0), (self.store.sent("alice")["total"], self.store.sent("bob")["total"]))
        # An identical retry of a request saved before retirement still returns it, without a notice.
        retry = self.cli("--as", "alice", "send", "bob", "--message", "Before", "--id", request_id)
        self.assertEqual((saved["id"], False), (retry["id"], retry["created"]))
        self.notify.assert_called_once()

    def test_saved_requests_of_a_retired_peer_can_still_be_answered(self):
        question = self.store.save("bob", "alice", "Question from bob")[0]
        incoming = self.store.save("alice", "bob", "Question for bob")[0]
        self.store.retire("bob")
        answer = self.cli("--as", "alice", "reply", question["id"], "--message", "Answer")
        self.notify.assert_not_called()
        self.assertEqual(("not_submitted", "recipient_retired"), (answer["submission"], answer["notification_detail"]))
        self.assertIn("recipient peer is retired", answer["next_action"])
        self.assertEqual("reply_received", self.store.get(question["id"])["state"])
        self.assertEqual("submitted", self.cli("--as", "bob", "reply", incoming["id"], "--message", "Late answer")["submission"])
        self.assertEqual([answer["id"]], [message["id"] for message in self.cli("--as", "bob", "inbox")["messages"]])
        self.assertIsNotNone(self.cli("--as", "bob", "ack", answer["id"])["ack_at"])

    def test_retirement_is_repeatable_and_keeps_the_registration(self):
        first = self.store.retire("bob")["retired_at"]
        self.assertEqual(first, self.cli("peer", "retire", "bob")["retired_at"])
        bob = self.store.peer("bob")
        address = ["--harness", "claude", "--session", bob["session_id"], "--workspace", str(self.path)]
        again = self.cli("peer", "add", "bob", *address)
        self.assertEqual(first, again["retired_at"])
        self.assertTrue(again["recovery"]["restore"].endswith("peer restore bob"))
        # Registration conflicts point to the list that includes the retired peer.
        taken = self.cli("peer", "add", "carol", *address, code=2)
        self.assertEqual(("session_already_has_a_peer_name", "bob", first), (taken["error"], taken["registered_peer"], taken["retired_at"]))
        self.assertTrue(taken["recovery"]["peers"].endswith("peer list --all"))
        self.assertIn("peer restore bob", taken["next_action"])
        rebound = self.cli("peer", "add", "bob", *address[:3], str(uuid.uuid4()), *address[4:], code=2)
        self.assertEqual(("peer_already_has_a_different_address", first), (rebound["error"], rebound["retired_at"]))
        self.assertTrue(rebound["recovery"]["peers"].endswith("peer list --all"))
        self.assertTrue(self.cli("peer", "add", "carol", "--harness", "claude", "--session", self.store.peer("alice")["session_id"],
                                 "--workspace", str(self.path), code=2)["recovery"]["peers"].endswith("peer list"))
        listed = self.cli("peer", "list")
        self.assertEqual((["alice"], 1), ([peer["name"] for peer in listed["peers"]], listed["retired_hidden"]))
        self.assertEqual([None, first], [peer["retired_at"] for peer in self.cli("peer", "list", "--all")["peers"]])
        with patch("harness_talk.cli.probe", return_value={"harness": "claude"}):
            self.assertEqual(first, self.cli("peer", "check", "bob")["retired_at"])
        with patch("harness_talk.cli.probe", side_effect=ValueError("recipient_unavailable")):
            self.assertEqual(first, self.cli("peer", "check", "bob", code=2)["retired_at"])
        for command in ("retire", "restore"):
            self.assertEqual("unknown_peer", self.cli("peer", command, "carol", code=2)["error"])

    def test_restore_allows_new_requests_without_replaying_skipped_notices(self):
        question = self.store.save("bob", "alice", "Question")[0]
        self.store.retire("bob")
        answer = self.store.save("alice", "bob", "Answer", in_reply_to=question["id"])[0]
        self.assertEqual("recipient_retired", self.store.notify_once(answer["id"], self.notify)["notification_detail"])
        for _ in range(2):
            self.assertIsNone(self.cli("peer", "restore", "bob")["retired_at"])
        self.assertEqual("recipient_retired", self.store.notify_once(answer["id"], self.notify)["notification_detail"])
        self.notify.assert_not_called()
        self.assertTrue(self.cli("--as", "alice", "send", "bob", "--message", "Welcome back")["created"])
        self.notify.assert_called_once()

    def test_final_check_skips_a_recipient_retired_during_preflight(self):
        question = self.store.save("bob", "alice", "Question")[0]
        answer = self.store.save("alice", "bob", "Answer", in_reply_to=question["id"])[0]
        with patch.object(adapters, "claude_socket", side_effect=lambda peer: (Store(self.store.path).retire("bob"), "/socket")[1]), \
                patch.object(adapters.socket, "socket") as socket:
            result = self.store.notify_once(answer["id"], adapters.notify)
            socket.return_value.__enter__.return_value.sendall.assert_not_called()
        self.assertEqual(("not_submitted", "recipient_retired"), (result["submission"], result["notification_detail"]))

    def test_databases_stay_usable_by_older_clients(self):
        # A database last opened by 0.3 has no retirement table; opening it with this version adds one.
        with closing(sqlite3.connect(self.store.path)) as db, db:
            db.execute("DROP TABLE retired_peers")
        Store(self.store.path).retire("bob")
        with closing(sqlite3.connect(self.store.path)) as db, db:
            self.assertEqual(2, db.execute("PRAGMA user_version").fetchone()[0])
            self.assertEqual(["name", "harness", "session_id", "workspace", "socket", "url"],
                             [row[1] for row in db.execute("PRAGMA table_info(peers)")])
            # The registration and request statements of htalk 0.3 still succeed.
            db.execute("INSERT INTO peers VALUES (?, ?, ?, ?, ?, ?)", ("carol", "claude", str(uuid.uuid4()), str(self.path), None, None))
            db.execute("""INSERT INTO messages (id, sender, recipient, in_reply_to, body, created_at, submission)
                VALUES (?, ?, ?, ?, ?, ?, 'not_submitted')""", (str(uuid.uuid4()), "alice", "bob", None, "Sent by 0.3", time.time()))
        self.assertEqual(["alice", "carol"], [peer["name"] for peer in self.cli("peer", "list")["peers"]])
        self.assertEqual(["Sent by 0.3"], [message["body"] for message in self.cli("--as", "bob", "inbox")["messages"]])


if __name__ == "__main__":
    unittest.main()
