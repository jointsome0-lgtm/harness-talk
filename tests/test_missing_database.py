import json
import os
from pathlib import Path
import sqlite3
import tempfile
import unittest
import uuid
from unittest.mock import patch

from harness_talk.cli import main
from harness_talk.store import Store


class MissingDatabase(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.path = Path(temporary.name)

    def cli(self, database, *words, code):
        with patch.dict(os.environ, {}, clear=True), patch("builtins.print") as output:
            self.assertEqual(code, main(["--db", str(database), *words]))
        return json.loads(output.call_args.args[0])

    def test_only_peer_add_creates_a_database(self):
        typo = self.path / "mistyped" / "mail.sqlite3"
        for words in (["peer", "list"], ["peer", "check", "bob"], ["--as", "alice", "inbox"]):
            error = self.cli(typo, *words, code=2)
            self.assertEqual(("database_not_found", str(typo.resolve())), (error["error"], error["resolved_path"]))
            self.assertNotIn("recovery", error)
            self.assertIn("HTALK_DB", error["next_action"])
        self.assertFalse(typo.parent.exists())
        self.cli(typo, "peer", "add", "bob", "--harness", "claude", "--session", str(uuid.uuid4()),
                 "--workspace", str(self.path), code=0)
        self.assertEqual(["bob"], [peer["name"] for peer in self.cli(typo, "peer", "list", code=0)["peers"]])

    def test_unreadable_or_corrupt_databases_keep_their_own_errors(self):
        corrupt = self.path / "corrupt.sqlite3"
        corrupt.write_bytes(b"not a database" * 100)
        self.assertNotEqual("database_not_found", self.cli(corrupt, "peer", "list", code=2)["error"])
        if os.geteuid() == 0:
            self.skipTest("root bypasses directory permissions")
        locked = self.path / "locked"
        locked.mkdir()
        Store(locked / "mail.sqlite3")
        locked.chmod(0)
        self.addCleanup(locked.chmod, 0o700)
        self.assertNotEqual("database_not_found", self.cli(locked / "mail.sqlite3", "peer", "list", code=2)["error"])

    def test_a_database_removed_after_opening_is_not_recreated(self):
        path = self.path / "mail.sqlite3"
        Store(path)
        store = Store(path, create=False)
        path.unlink()
        with self.assertRaises(sqlite3.OperationalError):
            store.peers()
        self.assertFalse(path.exists())


if __name__ == "__main__":
    unittest.main()
