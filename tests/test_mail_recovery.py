"""Saved answers remain readable under contention; send IDs retain retry identity."""
import json
import signal
import sqlite3
import subprocess
import unittest
import uuid

from compat_support import HtalkCase, wait_for
from test_mcp import McpClient


class MailRecovery(HtalkCase):
    def setUp(self):
        super().setUp()
        for peer in ("alice", "bob", "eve"):
            self.htalk("peer", "add", peer, "--harness", "generic", "--delivery", "pull")

    def finish_while_locked(self, process, code=0):
        try:
            stdout, stderr = process.communicate(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
            process.communicate()
            self.fail("the wait blocked on an optional mailbox write")
        self.assertEqual(code, process.returncode, stdout + stderr)
        return json.loads(stdout)

    def test_ready_answer_returns_while_writer_remains_locked(self):
        request = self.htalk("--as", "alice", "send", "bob", "--message", "Question")
        answer = self.htalk("--as", "bob", "reply", request["id"], "--message", "Answer")
        writer = sqlite3.connect(self.db)
        self.addCleanup(writer.close)
        writer.execute("BEGIN IMMEDIATE")
        try:
            for seconds in ("30", "0"):
                with self.subTest(seconds=seconds):
                    process = self.spawn("--as", "alice", "wait", request["id"],
                                         "--seconds", seconds)
                    # Completion while this writer is still held proves that neither
                    # registration nor the optional return marker waits for a write.
                    returned = self.finish_while_locked(process)
                    self.assertEqual(answer["id"], returned["reply"]["id"])
                    self.assertEqual("Answer", returned["reply"]["body"])
                    self.assertIsNone(returned["reply"]["ack_at"])
                    self.assertIsNone(self.kept(returned["reply"]["id"])["wait_returned_at"])
                    self.assertNotIn("wait_ended", returned)
                    self.assertEqual([], self.sql("SELECT * FROM waits"))
            self.error("--as", "bob", "wait", request["id"], "--seconds", "30",
                       error="wait_requires_own_request")
            self.error("--as", "alice", "wait", answer["id"], "--seconds", "30",
                       error="wait_requires_own_request")
            self.error("--as", "eve", "wait", request["id"], "--seconds", "30",
                       error="message_not_addressed_to_peer")
        finally:
            writer.rollback()
        returned = self.htalk("--as", "alice", "wait", request["id"], "--seconds", "30")
        self.assertIsNotNone(self.kept(returned["reply"]["id"])["wait_returned_at"])
        self.assertIsNone(returned["reply"]["ack_at"])

    def test_registered_wait_timeout_and_interrupt_do_not_block_on_cleanup(self):
        for interrupt in (False, True):
            with self.subTest(interrupt=interrupt):
                request = self.htalk("--as", "alice", "send", "bob", "--message", "Question")
                process = self.spawn("--as", "alice", "wait", request["id"], "--seconds",
                                     "30" if interrupt else "1")
                wait_for(lambda: self.sql("SELECT token FROM waits WHERE message_id=?",
                                          (request["id"],)), message="the wait registration")
                writer = sqlite3.connect(self.db)
                self.addCleanup(writer.close)
                writer.execute("BEGIN IMMEDIATE")
                try:
                    if interrupt:
                        process.send_signal(signal.SIGINT)
                    returned = self.finish_while_locked(process, code=130 if interrupt else 0)
                    if interrupt:
                        self.assertEqual("interrupted", returned["state"])
                    else:
                        self.assertEqual("timeout", returned["wait_ended"])
                        self.assertIsNone(returned["reply"])
                    # Cleanup could not write. Its stale hint must not prevent
                    # command completion; it never acknowledges the request.
                    self.assertEqual(1, self.sql("SELECT COUNT(*) FROM waits WHERE message_id=?",
                                                (request["id"],))[0][0])
                    self.assertIsNone(self.htalk("--as", "alice", "show", request["id"])["ack_at"])
                finally:
                    writer.rollback()

    def test_failed_marker_commit_does_not_claim_a_saved_receipt(self):
        request = self.htalk("--as", "alice", "send", "bob", "--message", "Question")
        answer = self.htalk("--as", "bob", "reply", request["id"], "--message", "Answer")
        reader = sqlite3.connect(self.db)
        self.addCleanup(reader.close)
        reader.execute("BEGIN")
        reader.execute("SELECT COUNT(*) FROM messages").fetchone()
        try:
            # In the rollback journal, this read transaction permits UPDATE to
            # begin but prevents its commit. The optional receipt must stay absent.
            process = self.spawn("--as", "alice", "wait", request["id"], "--seconds", "0")
            returned = self.finish_while_locked(process)
            self.assertEqual(answer["id"], returned["reply"]["id"])
            self.assertIsNone(self.kept(returned["reply"]["id"])["wait_returned_at"])
            self.assertIsNone(returned["reply"]["ack_at"])
        finally:
            reader.rollback()
        self.assertIsNone(self.kept(answer["id"])["wait_returned_at"])

    def test_cli_empty_id_is_rejected_and_omitted_id_is_generated(self):
        for _ in range(2):
            self.error("--as", "alice", "send", "bob", "--id", "", "--message", "Question",
                       error="invalid_uuid")
        self.assertEqual(0, self.sql("SELECT COUNT(*) FROM messages")[0][0])
        generated = self.htalk("--as", "alice", "send", "bob", "--message", "Generated")
        self.assertEqual(str(uuid.UUID(generated["id"])), generated["id"])
        chosen = str(uuid.uuid4())
        first = self.htalk("--as", "alice", "send", "bob", "--id", chosen, "--message", "Chosen")
        retry = self.htalk("--as", "alice", "send", "bob", "--id", chosen, "--message", "Chosen")
        self.assertEqual((chosen, True, False), (first["id"], first["created"], retry["created"]))
        self.assertEqual(first["id"], retry["id"])
        self.assertEqual(2, self.sql("SELECT COUNT(*) FROM messages")[0][0])

    def test_mcp_empty_id_is_rejected_and_valid_retry_is_preserved(self):
        client = McpClient(self)
        for _ in range(2):
            result = client.call("send", "bob", "--id", "", "--message", "Question")
            self.assertTrue(result["isError"])
            self.assertEqual(2, result["structuredContent"]["exit_code"])
            self.assertEqual("invalid_uuid",
                             result["structuredContent"]["result"]["error"])
        self.assertTrue(client.call("send", "bob", "--message", "No ID")["isError"])
        self.assertEqual(0, self.sql("SELECT COUNT(*) FROM messages")[0][0])
        chosen = str(uuid.uuid4())
        for created in (True, False):
            result = client.call("send", "bob", "--id", chosen, "--message", "Chosen")
            self.assertFalse(result.get("isError", False))
            saved = result["structuredContent"]["result"]
            self.assertEqual((chosen, created), (saved["id"], saved["created"]))
        self.assertEqual(1, self.sql("SELECT COUNT(*) FROM messages")[0][0])
        client.close()


if __name__ == "__main__":
    unittest.main()
