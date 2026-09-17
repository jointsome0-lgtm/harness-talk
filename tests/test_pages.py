from contextlib import closing
import json
import os
from pathlib import Path
import shlex
import tempfile
import time
import unittest
import uuid
from unittest.mock import patch

from harness_talk.cli import main
from harness_talk.store import Store


class Pages(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.path = Path(temporary.name)
        self.store = Store(self.path / "mail.sqlite3")
        for name in ("alice", "bob"):
            self.store.add_peer(name, "claude", str(uuid.uuid4()), self.path)

    def cli(self, *words, code=0):
        with patch.dict(os.environ, {}, clear=True), patch("builtins.print") as output:
            self.assertEqual(code, main(["--db", str(self.store.path), *words]))
        return json.loads(output.call_args.args[0])

    def follow(self, *words):
        """Run a listing and every recovery.next_page it returns."""
        pages = [self.cli(*words)]
        while "recovery" in pages[-1]:
            command = shlex.split(pages[-1]["recovery"]["next_page"])
            self.assertEqual(["htalk", "--db", str(self.store.path), "--as"], command[:4])
            pages.append(self.cli(*command[3:]))
        return pages

    def bulk_requests(self, count):
        # Direct inserts keep a history above the maximum page size fast to build.
        rows = [(str(uuid.uuid4()), "alice", "bob", f"Question {n}", time.time()) for n in range(count)]
        with closing(self.store.connect()) as db, db:
            db.executemany("""INSERT INTO messages (id, sender, recipient, body, created_at, submission)
                VALUES (?, ?, ?, ?, ?, 'not_submitted')""", rows)
        return [row[0] for row in rows]

    def test_sent_pages_cover_a_long_history_newest_first_without_gaps(self):
        ids = self.bulk_requests(520)
        pages = self.follow("--as", "alice", "sent", "--limit", "500")
        self.assertEqual([(500, 520, 20), (20, 520, 0)],
                         [(len(page["messages"]), page["total"], page["omitted"]) for page in pages])
        self.assertIn("--limit 500 --before-seq", pages[0]["recovery"]["next_page"])
        listed = [message["id"] for page in pages for message in page["messages"]]
        self.assertEqual(ids[::-1], listed)
        seqs = [message["seq"] for page in pages for message in page["messages"]]
        self.assertEqual(sorted(seqs, reverse=True), seqs)

    def test_inbox_pages_keep_open_questions_oldest_first(self):
        acknowledged = self.store.save("alice", "bob", "Acknowledged but unanswered")[0]
        self.store.ack(acknowledged["id"], "bob")
        answered = self.store.save("alice", "bob", "Answered")[0]
        self.store.save("bob", "alice", "Done", in_reply_to=answered["id"])
        open_ids = [acknowledged["id"], *self.bulk_requests(4)]
        pages = self.follow("--as", "bob", "inbox", "--limit", "2")
        self.assertEqual([(2, 5, 3), (2, 5, 1), (1, 5, 0)],
                         [(len(page["messages"]), page["total"], page["omitted"]) for page in pages])
        self.assertEqual(open_ids, [message["id"] for page in pages for message in page["messages"]])
        self.assertIn("--limit 2 --after-seq", pages[0]["recovery"]["next_page"])
        self.assertIn("recovery.next_page", pages[0]["next_action"])
        self.assertEqual("Acknowledged but unanswered", pages[0]["messages"][0]["body"])
        self.assertNotIn("recovery", pages[-1])

    def test_sent_summarizes_texts_of_requests_and_answers_unless_bodies_requested(self):
        body = "\n   \n  Привет — первая строка  \nвторая строка"
        question = self.store.save("alice", "bob", body)[0]
        self.store.save("bob", "alice", "Ответ\nподробности", in_reply_to=question["id"])
        long = self.store.save("alice", "bob", "я" * 200)[0]
        listed = {message["id"]: message for message in self.cli("--as", "alice", "sent")["messages"]}
        summary = listed[question["id"]]
        self.assertNotIn("body", summary)
        self.assertEqual((len(body.encode()), "Привет — первая строка"), (summary["body_bytes"], summary["body_preview"]))
        self.assertNotIn("body", summary["reply"])
        self.assertEqual("Ответ", summary["reply"]["body_preview"])
        self.assertEqual("я" * 120, listed[long["id"]]["body_preview"])
        self.assertEqual(body, self.cli("--as", "alice", "show", question["id"])["body"])
        full = self.cli("--as", "alice", "sent", "--bodies", "--limit", "1")
        self.assertEqual("я" * 200, full["messages"][0]["body"])
        self.assertIn("--bodies", full["recovery"]["next_page"])
        self.assertEqual("Ответ\nподробности", self.follow("--as", "alice", "sent", "--bodies", "--limit", "1")[1]["messages"][0]["reply"]["body"])

    def test_invalid_limits_and_cursors_are_json_errors(self):
        self.store.save("alice", "bob", "Question")
        for words, error in ((["sent", "--limit", "0"], "limit_must_be_between_1_and_500"),
                             (["inbox", "--limit", "501"], "limit_must_be_between_1_and_500"),
                             (["sent", "--before-seq", "0"], "seq_cursor_must_be_a_positive_integer"),
                             (["inbox", "--after-seq", str(2**63)], "seq_cursor_must_be_a_positive_integer")):
            self.assertEqual(error, self.cli("--as", "bob", *words, code=2)["error"])


if __name__ == "__main__":
    unittest.main()
