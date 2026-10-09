"""Black-box compatibility tests for the documented htalk CLI, JSON and SQLite contracts.

The suite runs the executable selected by HTALK_TEST_COMMAND (see compat_support).
Fake claude/codex executables, a fake Claude messaging socket and a loopback
OpenCode server stand in for clients. No test imports the implementation.
"""
import base64
from contextlib import closing
import errno
import json
import os
from pathlib import Path
import queue
import shlex
import signal
import socket
import sqlite3
import subprocess
import sys
import threading
import time
import unittest
import urllib.parse
import uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from compat_support import (KEPT, LOCK_TABLE, MESSAGE_KEYS, OPENCODE_CA, PEER_KEYS, ROW_KEYS, TIMEOUT,  # noqa: E402
                            UNSETTLED, INTERRUPT, HtalkCase, any_peer, gone, process_start, wait_for)

LEGACY_V1 = """
CREATE TABLE peers (name TEXT PRIMARY KEY, harness TEXT NOT NULL, session_id TEXT NOT NULL,
    workspace TEXT NOT NULL, socket TEXT, UNIQUE(harness, session_id));
CREATE TABLE messages (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT UNIQUE NOT NULL,
    sender TEXT NOT NULL REFERENCES peers(name), recipient TEXT NOT NULL REFERENCES peers(name),
    in_reply_to TEXT UNIQUE REFERENCES messages(id), body TEXT NOT NULL, created_at REAL NOT NULL, ack_at REAL,
    submission TEXT NOT NULL CHECK(submission IN ('not_submitted', 'submission_unknown', 'submitted')),
    notification_started_at REAL, notification_finished_at REAL, notification_detail TEXT);
PRAGMA user_version=1;
"""
PEER_COLUMNS = ["name", "harness", "session_id", "workspace", "socket", "url", "delivery"]
# Exit 3 when SQLite refuses a read at once because the database is busy.
READ_AT_ONCE = """import sqlite3, sys
try:
    sqlite3.connect(sys.argv[1], timeout=0).execute("SELECT id FROM messages").fetchall()
except sqlite3.OperationalError as exc:
    sys.exit(3 if exc.sqlite_errorcode == sqlite3.SQLITE_BUSY else 1)
"""


def turned_away(path):
    """Whether SQLite turns a new reader away, which it does while a writer waits to commit.
    Another process asks, without waiting: a connection of this one would share the lock held here."""
    asked = subprocess.run([sys.executable, "-c", READ_AT_ONCE, str(path)], capture_output=True, text=True,
                           timeout=TIMEOUT)
    assert asked.returncode in (0, 3), asked.stderr
    return asked.returncode == 3


def words_of(command):
    return shlex.split(command)


class DatabaseSelection(HtalkCase):
    def test_only_peer_add_creates_the_database(self):
        message_id = str(uuid.uuid4())
        commands = (["migrate"], ["peer", "list"], ["peer", "list", "--all"], ["peer", "check", "bob"], ["peer", "retire", "bob"],
                    ["peer", "restore", "bob"], ["inbox"], ["--as", "alice", "inbox"], ["--as", "alice", "sent"], ["--as", "alice", "watch"],
                    ["--as", "alice", "send", "bob", "--message", "Hello"],
                    ["--as", "alice", "reply", message_id, "--message", "Hello"],
                    ["--as", "alice", "show", message_id], ["--as", "alice", "ack", message_id],
                    ["--as", "alice", "wait", message_id, "--seconds", "0"])
        for words in commands:
            with self.subTest(words=words):
                error = self.error(*words, error="database_not_found")
                self.assertEqual(str(self.db), error["resolved_path"])
                self.assertNotIn("recovery", error)
                self.assertIn("HTALK_DB", error["next_action"])
                self.assertIn("peer add", error["next_action"])
        self.assertFalse(self.db.parent.exists())
        bob = self.add_peer("bob")
        self.assertEqual(any_peer(PEER_KEYS, PEER_KEYS | {"delivery"}), set(bob))
        self.assertEqual(["bob"], [peer["name"] for peer in self.htalk("peer", "list")["peers"]])
        # Created privately, independent of the caller's umask.
        self.assert_mode(0, self.db, 0o077)
        self.assert_mode(0, self.db.parent, 0o077)

    def test_path_precedence_and_resolution(self):
        option, env_db = self.tmp / "option.sqlite3", self.tmp / "env.sqlite3"
        xdg = self.tmp / "xdg"
        self.add_peer("bob", db=option, env={"HTALK_DB": str(env_db), "XDG_DATA_HOME": str(xdg)})
        self.assertEqual((True, False), (option.exists(), env_db.exists()))
        self.add_peer("bob", db=False, env={"HTALK_DB": str(env_db), "XDG_DATA_HOME": str(xdg)})
        self.assertTrue(env_db.exists())
        self.assertFalse(xdg.exists())
        self.add_peer("bob", db=False, env={"HTALK_DB": "", "XDG_DATA_HOME": str(xdg)})
        self.assertTrue((xdg / "harness-talk/mail.sqlite3").exists())
        self.add_peer("bob", db=False)
        self.assertTrue(self.default_db().exists())
        # An empty XDG_DATA_HOME is an unset one: the same mailbox, and nothing under the working directory.
        listed = self.htalk("peer", "list", db=False, env={"XDG_DATA_HOME": ""})
        self.assertEqual(["bob"], [peer["name"] for peer in listed["peers"]])
        self.assertFalse((self.tmp / "harness-talk").exists())
        # A relative or ~ path resolves against the working directory or HOME, in errors too.
        missing = self.error("peer", "list", db="rel/mail.sqlite3", cwd=self.work, error="database_not_found")
        self.assertEqual(str(self.work / "rel/mail.sqlite3"), missing["resolved_path"])
        self.add_peer("bob", db="~/tilde.sqlite3")
        self.assertTrue((self.home / "tilde.sqlite3").exists())
        self.assertFalse((self.tmp / "~").exists())
        self.assertEqual(["bob"], [p["name"] for p in self.htalk("peer", "list", db=self.home / "tilde.sqlite3")["peers"]])

    def test_a_path_with_a_space_and_letters_outside_ascii(self):
        odd = self.tmp / "почта и ящик"
        self.add_peer("bob", db=odd / "mail.sqlite3")
        self.assertEqual(["bob"], [peer["name"] for peer in self.htalk("peer", "list", db=odd / "mail.sqlite3")["peers"]])
        missing = self.error("peer", "list", db=odd / "none.sqlite3", error="database_not_found")
        self.assertEqual(str(odd / "none.sqlite3"), missing["resolved_path"])
        home = {"HOME": str(odd), "USERPROFILE": str(odd), "LOCALAPPDATA": str(odd / "local")}
        self.add_peer("dave", db=False, env=home)
        self.assertEqual(["dave"], [peer["name"] for peer in self.htalk("peer", "list", db=False, env=home)["peers"]])
        self.assertEqual(2, len(list(odd.rglob("mail.sqlite3"))))

    def test_existing_empty_corrupt_and_newer_files(self):
        self.db.parent.mkdir()
        self.db.touch()
        self.assertEqual({"peers": [], "retired_hidden": 0}, self.htalk("peer", "list"))
        self.assertEqual(3, self.user_version())
        corrupt = self.tmp / "corrupt.sqlite3"
        corrupt.write_bytes(b"not a database" * 100)
        error = self.error("peer", "list", db=corrupt)
        self.assertNotEqual("database_not_found", error["error"])
        self.assertEqual(b"not a database" * 100, corrupt.read_bytes())
        self.sql("PRAGMA user_version=4")
        self.error("peer", "list", error="unsupported_database_version")
        self.assertEqual("unsupported_database_version", self.add_peer("bob", code=2)["error"])
        self.assertEqual(4, self.user_version())


class SchemaCompatibility(HtalkCase):
    def legacy_v1(self, path):
        path.parent.mkdir(parents=True, exist_ok=True)
        with closing(sqlite3.connect(path)) as db, db:
            db.executescript(LEGACY_V1)
        ids = {name: str(uuid.uuid4()) for name in ("alice", "bob", "builder", "q1", "q2", "q3", "r3")}
        with closing(sqlite3.connect(path)) as db, db:
            db.executemany("INSERT INTO peers VALUES (?, ?, ?, ?, ?)", [
                ("alice", "claude", ids["alice"], str(self.work), None),
                ("bob", "claude", ids["bob"], str(self.work), None),
                ("builder", "codex", ids["builder"], str(self.work), "/nowhere/codex.sock")])
            rows = [(ids["q1"], "alice", "bob", None, "Acknowledged question", 1.0, 100.0, "submitted", 2.0, 3.0, "claude_socket_bytes_written"),
                    (ids["q2"], "alice", "bob", None, "Open question", 4.0, None, "not_submitted", None, None, None),
                    (ids["q3"], "bob", "alice", None, "Answered question", 5.0, None, "submission_unknown", 6.0, None, None),
                    (ids["r3"], "alice", "bob", ids["q3"], "Old answer", 7.0, None, "not_submitted", 8.0, 9.0, "recipient_unavailable")]
            db.executemany("""INSERT INTO messages (id, sender, recipient, in_reply_to, body, created_at, ack_at, submission,
                notification_started_at, notification_finished_at, notification_detail) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""", rows)
        return ids

    def assert_current_schema(self, path=None):
        self.assertEqual(3, self.user_version(path))
        self.assertEqual(PEER_COLUMNS, self.columns("peers", path))
        self.assertEqual(ROW_KEYS | set(KEPT), set(self.columns("messages", path)))
        self.assertEqual(["token", "message_id", "actor", "until"], self.columns("waits", path))
        self.assertEqual(["name", "retired_at"], self.columns("retired_peers", path))

    def backups(self, path=None):
        return sorted(Path(str(path or self.db) + ".backups").glob("*.sqlite3"))

    def snapshot(self, path):
        with closing(sqlite3.connect(path)) as db:
            return db.execute("PRAGMA user_version").fetchone()[0], list(db.iterdump())


    def test_version_1_database_migrates_and_preserves_rows(self):
        ids = self.legacy_v1(self.db)
        before = self.snapshot(self.db)
        result = self.htalk("peer", "list")
        self.assertEqual({"peers", "retired_hidden"}, set(result))
        listed = result["peers"]
        backup, = self.backups()
        self.assertEqual(before, self.snapshot(backup))
        self.assert_mode(0o600, backup)
        self.assert_mode(0o700, backup.parent)
        self.assert_current_schema()
        self.assertEqual(["alice", "bob", "builder"], [peer["name"] for peer in listed])
        self.assertEqual([None] * 3, [peer["url"] for peer in listed])
        self.assertEqual([None] * 3, [peer["retired_at"] for peer in listed])
        self.assertEqual("/nowhere/codex.sock", listed[2]["socket"])
        inbox = self.htalk("--as", "bob", "inbox")
        self.assertEqual([ids["q1"], ids["q2"], ids["r3"]], [m["id"] for m in inbox["messages"]])
        shown = self.htalk("--as", "alice", "show", ids["q1"])
        self.assertEqual((100.0, "submitted", "claude_socket_bytes_written", None),
                         (shown["ack_at"], shown["submission"], shown["notification_detail"], self.kept(shown["id"])["wait_returned_at"]))
        answered = self.htalk("--as", "bob", "show", ids["q3"])
        self.assertEqual(("reply_received", ids["r3"], "Old answer"), (answered["state"], answered["reply"]["id"], answered["reply"]["body"]))

    def test_identical_registration_of_a_migrated_peer_is_accepted_unchanged(self):
        ids = self.legacy_v1(self.db)
        alice = self.htalk("peer", "list")["peers"][0]
        self.assertEqual(alice, self.add_peer("alice", "claude", ids["alice"]))

    def test_early_version_two_migrates_on_ordinary_read(self):
        ids = self.legacy_v1(self.db)
        self.sql("ALTER TABLE peers ADD COLUMN url TEXT")
        self.sql("PRAGMA user_version=2")
        before = self.snapshot(self.db)
        self.htalk("--as", "bob", "show", ids["q1"])
        backup, = self.backups()
        self.assertEqual(before, self.snapshot(backup))
        self.assert_current_schema()
        self.assertIsNone(self.kept(ids["q1"])["wait_returned_at"])
        self.htalk("peer", "retire", "bob")
        self.assertEqual(2, len(self.htalk("peer", "list")["peers"]))

    def test_migrate_uses_normal_database_selection(self):
        selected = self.tmp / "selected.sqlite3"
        for path, env in ((selected, {"HTALK_DB": str(selected)}),
                          (self.default_db(), {})):
            with self.subTest(path=path):
                self.legacy_v1(path)
                before = self.snapshot(path)
                result = self.htalk("migrate", db=False, env=env)
                self.assertEqual({"state": "ready", "schema_version": 3, "resolved_path": str(path)}, result)
                backup, = self.backups(path)
                self.assertEqual(before, self.snapshot(backup))
                self.assert_current_schema(path)
                self.htalk("migrate", db=path)
                self.assertEqual([backup], self.backups(path))
                self.assertEqual(3, len(self.htalk("peer", "list", db=path)["peers"]))

    def test_broken_references_stop_before_backups_and_explain_failure(self):
        self.legacy_v1(self.db)
        self.sql("DELETE FROM peers WHERE name='bob'")
        before = self.db.read_bytes()
        for _ in range(2):
            error = self.error("peer", "list", error="database_foreign_key_violation")
            self.assertIn("foreign_key_check", error["next_action"])
            self.assertNotIn("recovery", error)
        self.assertEqual(before, self.db.read_bytes())
        self.assertFalse(Path(str(self.db) + ".backups").exists())

    def test_integrity_failure_stops_before_backups(self):
        self.legacy_v1(self.db)
        with closing(sqlite3.connect(self.db)) as db, db:
            db.execute("PRAGMA ignore_check_constraints=ON")
            db.execute("UPDATE messages SET submission='invalid'")
        before = self.db.read_bytes()
        for _ in range(2):
            error = self.error("peer", "list", error="database_integrity_check_failed")
            self.assertIn("integrity check failed", error["next_action"])
            self.assertNotIn("recovery", error)
        self.assertEqual(before, self.db.read_bytes())
        self.assertFalse(Path(str(self.db) + ".backups").exists())

    def test_interrupted_migration_rolls_back_before_commit(self):
        self.legacy_v1(self.db)
        before = self.db.read_bytes()
        with closing(sqlite3.connect(self.db)) as holder:
            holder.execute("BEGIN IMMEDIATE")
            process = self.spawn("migrate")
            time.sleep(0.2)
            self.assertIsNone(process.poll())
            process.send_signal(INTERRUPT)
            time.sleep(0.1)
            holder.rollback()
            self.finish(process, code=130)
        self.assertEqual(before, self.db.read_bytes())
        self.assertEqual(1, self.user_version())

    def test_interrupt_after_backup_rolls_back_and_retains_verified_copy(self):
        self.legacy_v1(self.db)
        before = self.snapshot(self.db)
        with closing(sqlite3.connect(self.db)) as reader:
            reader.execute("BEGIN")
            reader.execute("SELECT COUNT(*) FROM messages").fetchone()
            process = self.spawn("peer", "list")
            # A reader allows the backup but prevents the migration commit.
            wait_for(lambda: self.backups(), message="verified migration backup")
            self.assertIsNone(process.poll())
            process.send_signal(INTERRUPT)
            self.finish(process, code=130)
            reader.rollback()
        self.assertEqual(before, self.snapshot(self.db))
        backup, = self.backups()
        self.assertEqual(before, self.snapshot(backup))

    def test_send_and_reply_wait_after_migration_begins_without_replaying_it(self):
        for version in (1, 2):
            for command in ("send", "reply"):
                with self.subTest(version=version, command=command):
                    path = self.tmp / ("v%d-%s" % (version, command)) / "mail.sqlite3"
                    ids = self.legacy_v1(path)
                    if version == 2:
                        self.sql("ALTER TABLE peers ADD COLUMN url TEXT", path=path)
                        self.sql("PRAGMA user_version=2", path=path)
                    before = self.snapshot(path)
                    words = (["--as", "alice", "send", "bob", "--id", str(uuid.uuid4())]
                             if command == "send" else ["--as", "bob", "reply", ids["q2"]])
                    words += ["--message", "Migration commit contention", "--no-notify"]
                    with closing(sqlite3.connect(path)) as reader:
                        reader.execute("BEGIN")
                        reader.execute("SELECT COUNT(*) FROM messages").fetchone()
                        process = self.spawn(*words, db=path)

                        def migration_waits_to_commit():
                            if process.poll() is not None:
                                stdout, stderr = process.communicate(timeout=5)
                                self.fail("command exited before migration commit contention: %s\n%s\n%s"
                                          % (process.returncode, stdout, stderr))
                            # A verified backup requires successful migration BEGIN.
                            return self.backups(path) and turned_away(path)

                        wait_for(migration_waits_to_commit, message="migration backup and commit wait")
                        backup, = self.backups(path)
                        self.assertEqual(before, self.snapshot(backup))
                        self.assertEqual(version, reader.execute("PRAGMA user_version").fetchone()[0])
                        reader.rollback()
                    sent = self.finish(process)
                    self.assert_current_schema(path)
                    self.assertEqual([backup], self.backups(path), "migration was replayed")
                    self.assertEqual(5, len(self.sql("SELECT id FROM messages", path=path)))
                    self.assertEqual("Migration commit contention", sent["body"])
                    self.assertEqual(ids["q2"] if command == "reply" else None, sent["in_reply_to"])
                    repeated = self.htalk(*words, db=path)
                    self.assertEqual((False, sent["id"]), (repeated["created"], repeated["id"]))
                    self.assertEqual([("ok",)], self.sql("PRAGMA integrity_check", path=path))
                    self.assertEqual([], self.sql("PRAGMA foreign_key_check", path=path))
        self.assertEqual([], self.calls(), "--no-notify crossed a client boundary")

    def test_concurrent_first_opens_migrate_and_back_up_once(self):
        self.legacy_v1(self.db)
        before = self.snapshot(self.db)
        processes = [self.spawn("peer", "list") for _ in range(6)]
        for process in processes:
            self.assertEqual(3, len(self.finish(process)["peers"]))
        self.assert_current_schema()
        backup, = self.backups()
        self.assertEqual(before, self.snapshot(backup))
        self.htalk("migrate")
        self.assertEqual([backup], self.backups())

    def test_backup_failure_leaves_legacy_mailbox_unchanged(self):
        self.legacy_v1(self.db)
        before = self.db.read_bytes()
        blocked = Path(str(self.db) + ".backups")
        blocked.write_text("not a directory")
        error = self.error("peer", "list", error="database_backup_failed")
        self.assertEqual(str(blocked), error["backup_directory"])
        self.assertIn("not migrated", error["next_action"])
        self.assertNotIn("recovery", error)
        self.assertEqual(before, self.db.read_bytes())
        self.assertEqual("not a directory", blocked.read_text())

    def test_backup_includes_writer_that_commits_before_migration(self):
        for journal in ("delete", "wal"):
            with self.subTest(journal=journal):
                path = self.tmp / journal / "mail.sqlite3"
                ids = self.legacy_v1(path)
                with closing(sqlite3.connect(path)) as writer:
                    self.assertEqual(journal, writer.execute("PRAGMA journal_mode=" + journal).fetchone()[0])
                    writer.execute("BEGIN IMMEDIATE")
                    writer.execute("UPDATE messages SET body='Committed before migration' WHERE id=?", (ids["q1"],))
                    before = (writer.execute("PRAGMA user_version").fetchone()[0], list(writer.iterdump()))
                    process = self.spawn("peer", "list", db=path)
                    time.sleep(0.1)
                    self.assertIsNone(process.poll())
                    self.assertEqual([], self.backups(path))
                    writer.commit()
                    self.assertEqual(3, len(self.finish(process)["peers"]))
                backup, = self.backups(path)
                self.assertEqual(before, self.snapshot(backup))
                with closing(sqlite3.connect(backup)) as copied:
                    self.assertEqual("ok", copied.execute("PRAGMA integrity_check").fetchone()[0])
                    self.assertEqual("Committed before migration", copied.execute("SELECT body FROM messages WHERE id=?", (ids["q1"],)).fetchone()[0])
                self.assertEqual("Committed before migration", self.htalk("--as", "bob", "show", ids["q1"], db=path)["body"])


class GenericPeers(HtalkCase):
    def pull(self, name, harness="future-harness"):
        return self.htalk("peer", "add", name, "--delivery", "pull", "--harness", harness)

    def test_pull_exchange_does_not_probe_clients_and_preserves_receipts(self):
        alice = self.pull("alice", "codex")
        self.pull("bob", "claude")
        self.assertEqual("pull", alice["delivery"])
        self.assertIsNone(alice["session_id"])
        self.assertIsNone(alice["workspace"])
        self.assertEqual(alice, self.pull("alice", "codex"))
        self.assertEqual("pull_only", self.htalk("peer", "check", "bob")["status"])
        identity = {"CODEX_THREAD_ID": str(uuid.uuid4()), "CLAUDE_CODE_SESSION_ID": str(uuid.uuid4())}
        request = self.htalk("--as", "alice", "send", "bob", "--id", str(uuid.uuid4()), "--message", "Question", env=identity)
        self.assertEqual(("not_submitted", "pull_only", None, None),
                         (request["submission"], request["notification_detail"], *list(self.kept(request["id"]).values())[:2]))
        self.assertIsNone(self.htalk("--as", "bob", "show", request["id"])["ack_at"])
        self.htalk("--as", "bob", "ack", request["id"])
        self.assertEqual(1, self.htalk("--as", "bob", "inbox")["total"])
        reply = self.htalk("--as", "bob", "reply", request["id"], "--message", "Answer", env=identity)
        self.assertEqual("pull_only", reply["notification_detail"])
        got = self.htalk("--as", "alice", "wait", request["id"], "--seconds", "0")
        self.assertEqual(reply["id"], got["reply"]["id"])
        self.htalk("--as", "alice", "ack", reply["id"])
        self.assertEqual(0, self.htalk("--as", "alice", "inbox")["total"])
        self.assertEqual([], self.calls())

    def test_pull_recipient_can_reply_to_a_native_sender(self):
        _, listener = self.claude_recipient("alice")
        self.pull("bob", "hermes")
        request = self.htalk("--as", "alice", "send", "bob", "--message", "Question")
        self.assertEqual([], self.calls())
        reply = self.htalk("--as", "bob", "reply", request["id"], "--message", "Answer")
        self.assertEqual("submitted", reply["submission"])
        self.assertEqual(1, len(listener.frames()))
        self.assertEqual(reply["id"], self.htalk("--as", "bob", "reply", request["id"], "--message", "Answer")["id"])
        self.assertEqual(1, len(listener.frames()))
        self.htalk("--as", "alice", "ack", reply["id"])
        reverse = self.htalk("--as", "bob", "send", "alice", "--message", "Reverse")
        self.assertEqual("submitted", reverse["submission"])
        self.assertEqual(2, len(listener.frames()))
        self.htalk("--as", "alice", "ack", reverse["id"])
        self.htalk("peer", "retire", "bob")
        back = self.htalk("--as", "alice", "reply", reverse["id"], "--message", "Done")
        self.assertEqual(("alice", "bob", reverse["id"]), (back["sender"], back["recipient"], back["in_reply_to"]))
        self.assertEqual("pull_only", back["notification_detail"])
        self.assertIsNone(self.kept(back["id"])["notification_started_at"])
        cleanup = self.htalk("--as", "bob", "ack", back["id"])["notification_cleanup"]
        self.assertEqual("skipped", cleanup["status"])
        self.error("--as", "alice", "send", "bob", "--message", "Retired", error="peer_retired")
        self.htalk("peer", "restore", "bob")
        self.assertEqual(2, len(listener.frames()))

    def test_pull_rejects_native_address_flags_before_creating_database(self):
        for flag, value in (("--session", str(uuid.uuid4())), ("--workspace", str(self.work)),
                            ("--socket", "/tmp/test.sock"), ("--url", "http://127.0.0.1:4096")):
            with self.subTest(flag=flag):
                self.error("peer", "add", "bob", "--harness", "hermes", "--delivery", "pull", flag, value,
                           error="pull_peer_has_no_native_address")
                self.assertFalse(self.db.exists())

    def test_native_peer_without_its_session_is_a_usage_error(self):
        for words in ((), ("--delivery", "native"), ("--session", str(uuid.uuid4()))):
            with self.subTest(words=words):
                refused = self.run_raw("peer", "add", "bob", "--harness", "codex", *words)
                self.assertEqual((2, ""), (refused.code, refused.stdout))
                self.assertIn("required arguments were not provided", refused.stderr)
                self.assertIn("--workspace <WORKSPACE>", refused.stderr)
                self.assertRegex(refused.stderr, r"Usage: htalk(?i:\.exe)? peer add")
                self.assertFalse(self.db.exists())


    def test_unknown_native_adapter_keeps_mail_readable(self):
        self.pull("alice")
        self.add_peer("bob", "claude")
        self.sql("UPDATE peers SET harness='unknown-future-adapter' WHERE name='bob'")
        self.assertEqual(2, len(self.htalk("peer", "list")["peers"]))
        self.error("peer", "check", "bob", error="adapter_unavailable")
        message_id = str(uuid.uuid4())
        request = self.htalk("--as", "alice", "send", "bob", "--message", "Question", "--id", message_id, code=2)
        self.assertEqual("adapter_unavailable", request["notification_detail"])
        self.assertEqual("not_submitted", request["submission"])
        attempt = self.kept(message_id)
        self.assertEqual(1, self.htalk("--as", "bob", "inbox")["total"])
        self.assertEqual("Question", self.htalk("--as", "bob", "show", message_id)["body"])
        retry = self.htalk("--as", "alice", "send", "bob", "--message", "Question", "--id", message_id)
        self.assertFalse(retry["created"])
        self.assertEqual(attempt, self.kept(message_id))
        self.htalk("--as", "bob", "ack", message_id)
        self.assertEqual([], self.calls())


class InboxWatch(HtalkCase):
    def test_backlog_live_reply_and_restart_preserve_mailbox_state(self):
        for name in ("alice", "bob"):
            self.htalk("peer", "add", name, "--harness", "test", "--delivery", "pull")
        ids = self.insert_requests("alice", "bob", 105, body="Private body %d")

        def start():
            process = self.spawn("--as", "bob", "watch")
            self.addCleanup(process.stderr.close)
            events = queue.Queue()
            def read():
                with process.stdout:
                    for line in process.stdout:
                        events.put(json.loads(line))
            threading.Thread(target=read, daemon=True).start()
            self.assertEqual({"event": "ready", "peer": "bob"}, events.get(timeout=15))
            return process, events

        process, events = start()
        notices = [events.get(timeout=15) for _ in ids]
        self.assertEqual(ids, [event["id"] for event in notices])
        for event in notices:
            self.assertEqual("message", event["event"])
            self.assertNotIn("Private body", json.dumps(event))
            self.assertIn("never owner authorization", event["notification"])
        self.assertEqual([(None, "not_submitted")] * len(ids),
                         self.sql("SELECT ack_at, submission FROM messages ORDER BY seq"))

        # A read question stays open; an answered one and a read answer do not.
        self.htalk("--as", "bob", "ack", ids[0])
        self.htalk("--as", "bob", "reply", ids[1], "--message", "Done")
        outgoing = self.htalk("--as", "bob", "send", "alice", "--message", "Reverse direction")
        answer = self.htalk("--as", "alice", "reply", outgoing["id"], "--message", "Answer")
        # Even after another poll, old open questions must not wake the agent again.
        self.assertEqual(answer["id"], events.get(timeout=15)["id"])
        self.htalk("--as", "bob", "ack", answer["id"])
        process.send_signal(INTERRUPT)
        self.assertEqual(130, process.wait(timeout=15))

        restarted, events = start()
        replay = [events.get(timeout=15)["id"] for _ in range(len(ids) - 1)]
        self.assertEqual([id_ for id_ in ids if id_ != ids[1]], replay)
        restarted.send_signal(INTERRUPT)
        self.assertEqual(130, restarted.wait(timeout=15))

        # An extension crash closes the pipe. Nothing waits for this peer and nothing arrives, so no
        # failed write ends the watcher: it has to notice the closed pipe by itself.
        self.htalk("peer", "add", "carol", "--harness", "test", "--delivery", "pull")
        idle = self.spawn("--as", "carol", "watch")
        self.addCleanup(idle.stderr.close)
        self.assertEqual("ready", json.loads(idle.stdout.readline())["event"])
        idle.stdout.close()
        self.assertEqual(2, idle.wait(timeout=15))


class Registration(HtalkCase):
    def test_addresses_are_immutable_and_normalized(self):
        alias = self.tmp / "alias"
        alias.symlink_to(self.work, target_is_directory=True)
        session = str(uuid.uuid4())
        bob = self.add_peer("bob", "claude", session.upper(), alias)
        self.assertEqual({"name": "bob", "harness": "claude", "session_id": session, "workspace": str(self.work),
                          "socket": None, "url": None, "retired_at": None}, bob)
        self.assertEqual(bob, self.add_peer("bob", "claude", session, self.work))
        other = self.error("peer", "add", "bob", "--harness", "claude", "--session", str(uuid.uuid4()),
                           "--workspace", str(self.work), error="peer_already_has_a_different_address")
        self.assertEqual(("bob", session), (other["registered_peer"], other["registered_session_id"]))
        self.assertEqual(["peer", "list"], words_of(other["recovery"]["peers"])[3:])
        taken = self.error("peer", "add", "carol", "--harness", "claude", "--session", session,
                           "--workspace", str(self.work), error="session_already_has_a_peer_name")
        self.assertEqual(("bob", session), (taken["registered_peer"], taken["registered_session_id"]))
        # The same UUID under another client is a different address.
        self.assertEqual("codex", self.add_peer("carol", "codex", session)["harness"])
        socket_peer = self.add_peer("dave", "codex", None, None, "--socket", "rel/codex.sock")
        self.assertEqual(str(self.tmp / "rel/codex.sock"), socket_peer["socket"])
        self.assertEqual(["bob", "carol", "dave"], [peer["name"] for peer in self.htalk("peer", "list")["peers"]])


    def test_invalid_registrations_are_refused_without_saving(self):
        self.add_peer("keep")
        uuid_session, work = str(uuid.uuid4()), str(self.work)
        (self.tmp / "file").touch()
        coded = (("Bob", "claude", uuid_session, work, [], "invalid_peer_name"),
                 ("_bob", "claude", uuid_session, work, [], "invalid_peer_name"),
                 ("a" * 65, "claude", uuid_session, work, [], "invalid_peer_name"),
                 ("bo b", "claude", uuid_session, work, [], "invalid_peer_name"),
                 ("bö", "claude", uuid_session, work, [], "invalid_peer_name"),
                 ("bob", "claude", uuid_session, work, ["--socket", str(self.tmp / "c.sock")], "claude_socket_is_discovered_from_live_identity"),
                 ("bob", "claude", uuid_session, work, ["--url", "http://127.0.0.1:4096"], "url_is_only_for_opencode"),
                 ("bob", "codex", uuid_session, work, ["--url", "http://127.0.0.1:4096"], "url_is_only_for_opencode"),
                 ("bob", "opencode", "ses_x", work, ["--socket", str(self.tmp / "c.sock")], "opencode_uses_a_server_url_not_a_socket"),
                 ("bob", "opencode", "not-a-session", work, [], "invalid_opencode_session_id"),
                 ("bob", "opencode", "ses_x", work, ["--url", "http://example.com:4096"], "opencode_url_must_be_loopback"),
                 ("bob", "opencode", "ses_x", work, ["--url", "http://127.attacker.example:4096"], "opencode_url_must_be_loopback"),
                 ("bob", "opencode", "ses_x", work, ["--url", "http://10.0.0.1:4096"], "opencode_url_must_be_loopback"),
                 ("bob", "opencode", "ses_x", work, ["--url", "http://user:secret@127.0.0.1:4096"], "invalid_opencode_url"),
                 ("bob", "opencode", "ses_x", work, ["--url", "ftp://127.0.0.1:4096"], "invalid_opencode_url"),
                 ("bob", "opencode", "ses_x", work, ["--url", "http://127.0.0.1:4096/?token=1"], "invalid_opencode_url"),
                 *(("bob", "opencode", "ses_x", work, ["--url", "http://%s:4096" % host], "opencode_url_must_be_loopback")
                   for host in ("127.1", "2130706433", "127.0.0.%31", "ｌｏｃａｌｈｏｓｔ", "localhost.example", "192.168.1.1")),
                 *(("bob", "opencode", "ses_x", work, ["--url", url], "invalid_opencode_url")
                   for url in ("http://127.0.0.1:4096\\path", "http://127.0.0.1:99999", "http://user@localhost", "http://localhost/?x")),
                 ("bob", "claude", uuid_session, str(self.tmp / "file"), [], "workspace_must_be_a_directory"))
        for name, harness, session, workspace, options, code in coded:
            with self.subTest(name=name, harness=harness, options=options):
                if harness != "opencode" and code != "invalid_peer_name":
                    self.need_native()
                error = self.error("peer", "add", name, "--harness", harness, "--session", session,
                                   "--workspace", workspace, *options, error=code)
                self.assertIn("peer list", error["recovery"]["peers"])
                self.assertNotIn("secret", json.dumps(error))
        # Exception text for these is not a documented code; only the failure is.
        for session, workspace in (("not-a-uuid", work), (uuid_session, str(self.tmp / "missing"))):
            with self.subTest(session=session, workspace=workspace):
                self.error("peer", "add", "bob", "--harness", "claude", "--session", session, "--workspace", workspace)
        # A loopback URL is saved as written, without the parts a request never carries.
        for index, (url, saved) in enumerate((("HTTP://LOCALHOST:80/a/", "http://LOCALHOST:80/a"), ("http://[::1]:4096/", "http://[::1]:4096"),
                                              (" http://127.0.0.1:4096/a\tb", "http://127.0.0.1:4096/ab"))):
            self.assertEqual(saved, self.add_peer("url%d" % index, "opencode", None, None, "--url", url)["url"])
            self.htalk("peer", "retire", "url%d" % index)
        self.assertEqual("a" * 64, self.add_peer("a" * 64)["name"])
        self.assertEqual(["a" * 64, "keep"], [peer["name"] for peer in self.htalk("peer", "list")["peers"]])


class Conversation(HtalkCase):
    def setUp(self):
        super().setUp()
        for name in ("alice", "bob", "eve"):
            self.add_peer(name)

    def test_request_read_question_and_answer_states_are_separate(self):
        question = self.htalk("--as", "alice", "send", "bob", "--message", "Which case?", "--no-notify")
        self.assertEqual(MESSAGE_KEYS | {"created", "actor_source"}, set(question))
        self.assertEqual(("alice", "bob", None, "Which case?", "saved", None, True, "option"),
                         (question["sender"], question["recipient"], question["in_reply_to"], question["body"],
                          question["state"], question["reply"], question["created"], question["actor_source"]))
        self.assertEqual(("not_submitted", None, any_peer(None, "pull_only"), None, None),
                         (question["submission"], self.kept(question["id"])["notification_started_at"], question["notification_detail"],
                          question["ack_at"], self.kept(question["id"])["wait_returned_at"]))
        self.assertEqual(str(uuid.UUID(question["id"])), question["id"])
        qid = question["id"]
        inbox = self.htalk("--as", "bob", "inbox")
        self.assertEqual(([qid], 1, 0), ([m["id"] for m in inbox["messages"]], inbox["total"], inbox["omitted"]))
        self.assertEqual("Which case?", inbox["messages"][0]["body"])
        self.assertEqual(({"messages", "total", "omitted", "actor_source"}, MESSAGE_KEYS),
                         (set(inbox), set(inbox["messages"][0])))
        self.error("--as", "eve", "show", qid, error="message_not_addressed_to_peer")
        self.error("--as", "alice", "show", str(uuid.uuid4()), error="unknown_message")
        for actor in ("alice", "eve"):
            self.error("--as", actor, "ack", qid, error="only_recipient_can_ack")
        self.assertIsNone(self.htalk("--as", "alice", "show", qid)["ack_at"])
        # Acknowledging records reading only; the question stays open.
        read = self.htalk("--as", "bob", "ack", qid)
        self.assertIsNotNone(read["ack_at"])
        self.assertEqual(("saved", None), (read["state"], read["reply"]))
        self.assertEqual(MESSAGE_KEYS | {"notification_cleanup", "actor_source"}, set(read))
        self.assertEqual(read["ack_at"], self.htalk("--as", "bob", "ack", qid)["ack_at"])
        self.assertEqual([qid], [m["id"] for m in self.htalk("--as", "bob", "inbox")["messages"]])
        # Only the recipient answers, and only a request.
        self.error("--as", "eve", "reply", qid, "--message", "Forged", error="message_not_addressed_to_peer")
        self.error("--as", "alice", "reply", qid, "--message", "Self", error="sender_and_recipient_must_differ")
        answer = self.htalk("--as", "bob", "reply", qid, "--message", "This one.", "--no-notify")
        self.assertEqual(("bob", "alice", qid, True, "saved"),
                         (answer["sender"], answer["recipient"], answer["in_reply_to"], answer["created"], answer["state"]))
        self.assertNotEqual(qid, answer["id"])
        self.assertEqual([], self.htalk("--as", "bob", "inbox")["messages"])
        self.assertEqual([answer["id"]], [m["id"] for m in self.htalk("--as", "alice", "inbox")["messages"]])
        again = self.htalk("--as", "bob", "reply", qid, "--message", "This one.", "--no-notify")
        self.assertEqual((answer["id"], False), (again["id"], again["created"]))
        conflict = self.error("--as", "bob", "reply", qid, "--message", "Changed", error="reply_conflict_existing_answer_preserved")
        self.assertEqual(qid, conflict["message_id"])
        self.assertEqual("This one.", self.recover(conflict["recovery"]["show"])["reply"]["body"])
        self.assertIn("sent", words_of(conflict["recovery"]["sent"]))
        self.error("--as", "alice", "reply", answer["id"], "--message", "Nested", error="reply_requires_a_request")
        shown = self.htalk("--as", "alice", "show", qid)
        self.assertEqual(("reply_received", ROW_KEYS), (shown["state"], set(shown["reply"])))
        self.assertEqual(read["ack_at"], shown["ack_at"])
        # wait returns the answer and records that, without acknowledging it.
        self.error("--as", "bob", "wait", qid, "--seconds", "0", error="wait_requires_own_request")
        self.error("--as", "alice", "wait", answer["id"], "--seconds", "0", error="wait_requires_own_request")
        waited = self.htalk("--as", "alice", "wait", qid, "--seconds", "0")
        self.assertEqual(answer["id"], waited["reply"]["id"])
        self.assertIsNotNone(self.kept(waited["reply"]["id"])["wait_returned_at"])
        self.assertIsNone(waited["reply"]["ack_at"])
        self.assertNotIn("wait_ended", waited)
        self.assertEqual([answer["id"]], [m["id"] for m in self.htalk("--as", "alice", "inbox")["messages"]])
        self.assertIsNotNone(self.htalk("--as", "alice", "ack", answer["id"])["ack_at"])
        self.assertEqual([], self.htalk("--as", "alice", "inbox")["messages"])
        self.assertEqual([], self.waits())

    def test_caller_ids_and_identical_retries(self):
        chosen = str(uuid.uuid4())
        first = self.htalk("--as", "alice", "send", "bob", "--id", chosen.upper(), "--message", "Q", "--no-notify")
        self.assertEqual((chosen, True), (first["id"], first["created"]))
        retry = self.htalk("--as", "alice", "send", "bob", "--id", chosen, "--message", "Q", "--no-notify")
        self.assertEqual((chosen, False, first["created_at"]), (retry["id"], retry["created"], retry["created_at"]))
        for words in (["send", "bob", "--id", chosen, "--message", "Changed"],
                      ["send", "eve", "--id", chosen.upper(), "--message", "Q"]):
            with self.subTest(words=words):
                conflict = self.error("--as", "alice", *words, "--no-notify", error="message_id_conflict")
                self.assertEqual(chosen, conflict["message_id"])
                self.assertEqual("Q", self.recover(conflict["recovery"]["show"])["body"])
                self.assertIn("Do not resend", conflict["next_action"])
        # An ID used in a conversation this peer cannot read is not revealed.
        hidden = self.htalk("--as", "eve", "send", "bob", "--message", "Private", "--no-notify")["id"]
        conflict = self.error("--as", "alice", "send", "bob", "--id", hidden, "--message", "Q", "--no-notify", error="message_id_conflict")
        self.assertNotIn("show", conflict["recovery"])
        self.assertIn("not readable by this peer", conflict["next_action"])
        self.assertNotIn(hidden, [m["id"] for m in self.recover(conflict["recovery"]["sent"])["messages"]])
        self.error("--as", "alice", "send", "bob", "--id", "not-a-uuid", "--message", "Q", "--no-notify")
        self.error("--as", "alice", "send", "alice", "--message", "Q", "--no-notify", error="sender_and_recipient_must_differ")
        self.error("--as", "alice", "send", "zed", "--message", "Q", "--no-notify", error="unknown_peer")
        self.assertEqual(1, self.htalk("--as", "alice", "sent")["total"])

    def test_wait_timeout_leaves_the_request_saved(self):
        question = self.htalk("--as", "alice", "send", "bob", "--message", "Q", "--no-notify")
        for seconds in ("0", "0.2"):
            with self.subTest(seconds=seconds):
                waited = self.htalk("--as", "alice", "wait", question["id"], "--seconds", seconds)
                self.assertEqual(("timeout", "saved", None), (waited["wait_ended"], waited["state"], waited["reply"]))
                self.assertEqual(set(), UNSETTLED & set(waited))
        self.assertEqual([], self.waits())
        timed = self.htalk("--as", "alice", "send", "bob", "--message", "Timed", "--no-notify", "--wait", "0.2")
        self.assertEqual(("timeout", True), (timed["wait_ended"], timed["created"]))


    def test_message_bodies_and_limits(self):
        accepted = ("a" * 32000, "я" * 16000, "  padded  \n\n", "🙂 multi\nline\n")
        for body in accepted:
            saved = self.htalk("--as", "alice", "send", "bob", "--message", body, "--no-notify")
            self.assertEqual(body, self.htalk("--as", "bob", "show", saved["id"])["body"])
        for body in ("a" * 32001, "я" * 16000 + "a", "", "   \n\t "):
            with self.subTest(size=len(body.encode())):
                self.error("--as", "alice", "send", "bob", "--message", body, "--no-notify", error="message_must_be_1_to_32000_bytes")
        text = self.tmp / "message.txt"
        text.write_text("First line\nВторая строка\n\nartifact: /tmp/x\n", encoding="utf-8")
        from_file = self.htalk("--as", "alice", "send", "bob", "--message-file", str(text), "--no-notify")
        self.assertEqual("First line\nВторая строка\n\nartifact: /tmp/x\n", from_file["body"])
        binary = self.tmp / "binary.txt"
        binary.write_bytes(b"\xff\xfe\x00bad")
        for path in (self.tmp / "missing.txt", binary):
            with self.subTest(path=path.name):
                self.error("--as", "alice", "send", "bob", "--message-file", str(path), "--no-notify")
        self.assertEqual(len(accepted) + 1, self.htalk("--as", "alice", "sent")["total"])
        question = self.htalk("--as", "alice", "send", "bob", "--message", "Q", "--no-notify")["id"]
        for seconds in ("46", "-1", "-0.5", "nan", "inf"):
            with self.subTest(seconds=seconds):
                self.error("--as", "alice", "send", "bob", "--message", "Never saved", "--wait", seconds,
                           error="wait_seconds_must_be_between_0_and_45")
                self.error("--as", "alice", "wait", question, "--seconds", seconds, error="wait_seconds_must_be_between_0_and_45")
        self.assertEqual(len(accepted) + 2, self.htalk("--as", "alice", "sent")["total"])


class Listings(HtalkCase):
    def setUp(self):
        super().setUp()
        for name in ("alice", "bob"):
            self.add_peer(name)

    def follow(self, *words):
        pages = [self.htalk(*words)]
        while "next_page" in pages[-1]:
            self.assertEqual(set(), UNSETTLED & set(pages[-1]))
            pages.append(self.recover(pages[-1]["next_page"]))
        return pages

    def test_sent_pages_newest_first_without_gaps(self):
        ids = self.insert_requests("alice", "bob", 25)
        pages = self.follow("--as", "alice", "sent", "--limit", "10")
        self.assertEqual([(10, 25, 15), (10, 25, 5), (5, 25, 0)],
                         [(len(p["messages"]), p["total"], p["omitted"]) for p in pages])
        self.assertEqual(ids[::-1], [m["id"] for p in pages for m in p["messages"]])
        last = pages[0]["messages"][-1]["seq"]
        self.assertEqual(["htalk", "--db", str(self.db), "--as", "alice", "sent", "--limit", "10", "--before-seq", str(last)],
                         words_of(pages[0]["next_page"]))
        seqs = [m["seq"] for p in pages for m in p["messages"]]
        self.assertEqual(sorted(seqs, reverse=True), seqs)
        default = self.htalk("--as", "alice", "sent")
        self.assertEqual((20, 5), (len(default["messages"]), default["omitted"]))
        older = self.htalk("--as", "alice", "sent", "--before-seq", str(seqs[-5]))
        self.assertEqual((ids[3::-1], 25, 0), ([m["id"] for m in older["messages"]], older["total"], older["omitted"]))
        bodies = self.follow("--as", "alice", "sent", "--limit", "20", "--bodies")
        self.assertIn("--bodies", words_of(bodies[0]["next_page"]))
        self.assertEqual("Question 0", bodies[-1]["messages"][-1]["body"])

    def test_inbox_pages_oldest_first_and_keep_open_work(self):
        acknowledged = self.htalk("--as", "alice", "send", "bob", "--message", "Read, unanswered", "--no-notify")["id"]
        self.htalk("--as", "bob", "ack", acknowledged)
        answered = self.htalk("--as", "alice", "send", "bob", "--message", "Answered", "--no-notify")["id"]
        answer = self.htalk("--as", "bob", "reply", answered, "--message", "Done", "--no-notify")["id"]
        unread = self.htalk("--as", "alice", "send", "bob", "--message", "Unread", "--no-notify")["id"]
        open_ids = [acknowledged, unread, *self.insert_requests("alice", "bob", 3)]
        pages = self.follow("--as", "bob", "inbox", "--limit", "2")
        self.assertEqual([(2, 5, 3), (2, 5, 1), (1, 5, 0)], [(len(p["messages"]), p["total"], p["omitted"]) for p in pages])
        self.assertEqual(open_ids, [m["id"] for p in pages for m in p["messages"]])
        self.assertEqual(["htalk", "--db", str(self.db), "--as", "bob", "inbox", "--limit", "2", "--after-seq",
                          str(pages[0]["messages"][-1]["seq"])], words_of(pages[0]["next_page"]))
        self.assertNotIn("next_page", pages[-1])
        after = self.htalk("--as", "bob", "inbox", "--after-seq", str(pages[1]["messages"][-1]["seq"]))
        self.assertEqual(open_ids[4:], [m["id"] for m in after["messages"]])
        self.assertEqual((5, 0), (after["total"], after["omitted"]))
        # Unacknowledged answers are incoming work for the requester.
        self.assertEqual([answer], [m["id"] for m in self.htalk("--as", "alice", "inbox")["messages"]])


class Recovery(HtalkCase):
    def test_recovery_commands_are_executable_with_a_quoted_database_path(self):
        self.db = self.tmp / "mail 'quoted' $draft" / "m.sqlite3"
        self.add_peer("builder")
        self.add_peer("reviewer", "claude")
        # Nobody runs the reviewer's client, so the notice fails and the message is answered with advice.
        question = self.htalk("--as", "builder", "send", "reviewer", "--message", "Question", code=2)
        prefix = ["htalk", "--db", str(self.db), "--as", "builder"]
        self.assertEqual(prefix + ["show", question["id"]], words_of(question["recovery"]["show"]))
        self.assertEqual({"show", "wait"}, set(question["recovery"]))
        self.assertIn("recovery.wait", question["next_action"])
        self.assertEqual("timeout", self.recover(question["recovery"]["wait"].replace("--seconds 45", "--seconds 0"))["wait_ended"])
        incoming = self.htalk("--as", "reviewer", "inbox")["messages"][0]
        self.assertEqual({"show", "ack_after_reading"}, set(incoming["recovery"]))
        self.assertIsNotNone(self.recover(incoming["recovery"]["ack_after_reading"])["ack_at"])
        self.assertEqual({"show"}, set(self.recover(incoming["recovery"]["show"])["recovery"]))
        self.htalk("--as", "reviewer", "reply", question["id"], "--message", "Answer", "--no-notify")
        recovered = self.htalk("--as", "builder", "sent")["messages"][0]
        self.assertEqual({"show", "show_reply", "ack_after_reading"}, set(recovered["recovery"]))
        read = self.recover(recovered["recovery"]["show_reply"])
        self.assertEqual(("Answer", question["id"]), (read["body"], read["in_reply_to"]))
        acked = self.recover(recovered["recovery"]["ack_after_reading"])
        self.assertEqual(read["id"], acked["id"])
        self.assertIsNotNone(acked["ack_at"])
        self.assertEqual({"show", "show_reply"}, set(self.htalk("--as", "builder", "show", question["id"])["recovery"]))
        for command in (question["recovery"]["show"], recovered["recovery"]["show_reply"]):
            self.assertEqual(prefix, words_of(command)[:5])
        error = self.error("--as", "builder", "send", "nobody", "--message", "x", "--no-notify", error="unknown_peer")
        self.assertEqual({"peers", "sent", "inbox"}, set(error["recovery"]))
        self.assertEqual(2, len(self.recover(error["recovery"]["peers"])["peers"]))
        self.assertEqual(1, self.recover(error["recovery"]["sent"])["total"])
        self.assertEqual(0, self.recover(error["recovery"]["inbox"])["total"])


class Retirement(HtalkCase):
    def test_retired_peers_neither_send_nor_receive_new_requests(self):
        self.add_peer("alice")
        bob = self.add_peer("bob", "claude")
        chosen = str(uuid.uuid4())
        self.htalk("--as", "alice", "send", "bob", "--id", chosen, "--message", "Before", "--no-notify")
        from_bob = self.htalk("--as", "bob", "send", "alice", "--message", "Question from bob", "--no-notify")["id"]
        retired = self.htalk("peer", "retire", "bob")
        self.assertEqual(PEER_KEYS, set(retired))
        first = retired["retired_at"]
        self.assertIsNotNone(first)
        self.assertEqual(first, self.htalk("peer", "retire", "bob")["retired_at"])
        for sender, recipient in (("alice", "bob"), ("bob", "alice")):
            with self.subTest(sender=sender):
                refused = self.error("--as", sender, "send", recipient, "--message", "After", error="peer_retired")
                self.assertEqual(["bob"], refused["retired_peers"])
                self.assertIn("Nothing was saved", refused["next_action"])
        self.assertEqual((1, 1), (self.htalk("--as", "alice", "sent")["total"], self.htalk("--as", "bob", "sent")["total"]))
        retry = self.htalk("--as", "alice", "send", "bob", "--id", chosen, "--message", "Before", "--no-notify")
        self.assertEqual((chosen, False), (retry["id"], retry["created"]))
        # Saved requests are still answered; a notice to the retired recipient is skipped with exit 0.
        answer = self.htalk("--as", "alice", "reply", from_bob, "--message", "Answer")
        self.assertEqual(("not_submitted", "recipient_retired"), (answer["submission"], answer["notification_detail"]))
        self.assertEqual(set(), UNSETTLED & set(answer))
        self.assertEqual([], self.calls("claude"))
        self.assertTrue(self.htalk("--as", "bob", "reply", chosen, "--message", "Late", "--no-notify")["created"])
        self.assertEqual([answer["id"]], [m["id"] for m in self.htalk("--as", "bob", "inbox")["messages"]])
        self.assertEqual("reply_received", self.htalk("--as", "bob", "wait", from_bob, "--seconds", "0")["state"])
        self.assertIsNotNone(self.htalk("--as", "bob", "ack", answer["id"])["ack_at"])
        # Listing and registration keep the binding visible.
        listed = self.htalk("peer", "list")
        self.assertEqual((["alice"], 1), ([p["name"] for p in listed["peers"]], listed["retired_hidden"]))
        everyone = self.htalk("peer", "list", "--all")
        self.assertNotIn("retired_hidden", everyone)
        self.assertEqual([None, first], [p["retired_at"] for p in everyone["peers"]])
        self.assertEqual(first, self.error("peer", "check", "bob", error="recipient_unavailable")["retired_at"])
        again = self.add_peer("bob", "claude", bob["session_id"])
        self.assertEqual(first, again["retired_at"])
        self.assertEqual(["peer", "restore", "bob"], words_of(again["recovery"]["restore"])[3:])
        taken = self.error("peer", "add", "carol", "--harness", "claude", "--session", bob["session_id"],
                           "--workspace", str(self.work), error="session_already_has_a_peer_name")
        self.assertEqual(("bob", first), (taken["registered_peer"], taken["retired_at"]))
        self.assertEqual(["peer", "list", "--all"], words_of(taken["recovery"]["peers"])[3:])
        self.assertIn("peer restore bob", taken["next_action"])
        rebound = self.error("peer", "add", "bob", "--harness", "claude", "--session", str(uuid.uuid4()),
                             "--workspace", str(self.work), error="peer_already_has_a_different_address")
        self.assertEqual(first, rebound["retired_at"])
        self.assertEqual(["peer", "list", "--all"], words_of(rebound["recovery"]["peers"])[3:])
        # Restoring allows new requests but does not replay skipped notices.
        for _ in range(2):
            self.assertIsNone(self.htalk("peer", "restore", "bob")["retired_at"])
        self.assertEqual("recipient_retired", self.htalk("--as", "alice", "show", answer["id"])["notification_detail"])
        self.assertTrue(self.htalk("--as", "alice", "send", "bob", "--message", "Welcome back", "--no-notify")["created"])
        self.assertEqual(["alice", "bob"], [p["name"] for p in self.htalk("peer", "list")["peers"]])
        for command in ("retire", "restore"):
            self.error("peer", command, "carol", error="unknown_peer")


class ActorSelection(HtalkCase):
    def test_option_environment_and_missing_name(self):
        self.add_peer("alice")
        self.add_peer("bob")
        self.assertEqual("HTALK_PEER", self.htalk("inbox", env={"HTALK_PEER": "alice"})["actor_source"])
        chosen = self.htalk("--as", "bob", "send", "alice", "--message", "Q", "--no-notify", env={"HTALK_PEER": "alice"})
        self.assertEqual(("bob", "option"), (chosen["sender"], chosen["actor_source"]))
        missing = self.error("inbox", error="peer_required_use_as_or_HTALK_PEER")
        self.assertEqual({"harness": "claude", "status": "unrecognized", "reason": "claude_session_variables_absent"},
                         missing["native_session"])
        self.assertIn("--as NAME", missing["next_action"])
        self.assertEqual(2, len(self.recover(missing["recovery"]["peers"])["peers"]))
        unknown = self.error("--as", "zed", "inbox", error="unknown_peer")
        self.assertEqual("option", unknown["actor_source"])

    def test_codex_sender_must_match_codex_thread_id(self):
        builder = self.add_peer("builder", "codex")
        self.add_peer("reviewer")
        other = str(uuid.uuid4())
        for words, extra, source in ((["--as", "builder"], {}, "option"), ([], {"HTALK_PEER": "builder"}, "HTALK_PEER")):
            with self.subTest(source=source):
                conflict = self.error(*words, "inbox", env={"CODEX_THREAD_ID": other, **extra},
                                      error="actor_conflicts_with_CODEX_THREAD_ID")
                self.assertEqual(("builder", builder["session_id"], other, source),
                                 (conflict["registered_peer"], conflict["registered_session_id"],
                                  conflict["current_session_id"], conflict["actor_source"]))
                self.assertEqual(["--as", "builder", "inbox"], words_of(conflict["recovery"]["inbox_in_registered_session"])[3:])
                self.assertIn("peers", conflict["recovery"])
        self.htalk("--as", "builder", "inbox", env={"CODEX_THREAD_ID": builder["session_id"]})
        self.htalk("--as", "reviewer", "inbox", env={"CODEX_THREAD_ID": other})

    def test_recognized_claude_session_selects_its_peer(self):
        session = str(uuid.uuid4())
        native = self.native_claude(session)
        builder = self.add_peer("builder")
        self.add_peer("coder", "codex")
        unregistered = self.error("inbox", env=native, error="peer_required_use_as_or_HTALK_PEER")
        self.assertEqual({"harness": "claude", "status": "recognized", "session_id": session, "workspace": str(self.work)},
                         unregistered["native_session"])
        self.assertIn("peer add", unregistered["next_action"])
        self.add_peer("reviewer", "claude", session)
        # Nobody runs the builder's client, so the notice fails and the message is answered with advice.
        question = self.htalk("send", "builder", "--message", "Question", env=native, code=2)
        self.assertEqual(("reviewer", "native_session"), (question["sender"], question["actor_source"]))
        self.assertEqual(["--as", "reviewer", "show", question["id"]], words_of(question["recovery"]["show"])[3:])
        self.assertEqual("option", self.recover(question["recovery"]["show"], env=native)["actor_source"])
        self.assertEqual("HTALK_PEER", self.htalk("inbox", env={**native, "HTALK_PEER": "reviewer"})["actor_source"])
        for words, extra, source in ((["--as", "builder"], {}, "option"), ([], {"HTALK_PEER": "builder"}, "HTALK_PEER")):
            with self.subTest(source=source):
                conflict = self.error(*words, "inbox", env={**native, **extra}, error="actor_conflicts_with_CLAUDE_CODE_SESSION_ID")
                self.assertEqual((builder["session_id"], session, source),
                                 (conflict["registered_session_id"], conflict["current_session_id"], conflict["actor_source"]))
                self.assertIn("--as builder", conflict["recovery"]["inbox_in_registered_session"])
        # The check does not apply to peers of other clients.
        self.assertEqual("option", self.htalk("--as", "coder", "inbox", env=native)["actor_source"])
        # A retired peer is still selected for its own session.
        self.htalk("peer", "retire", "reviewer")
        self.assertEqual("native_session", self.htalk("inbox", env=native)["actor_source"])

    def test_unrecognized_claude_evidence_falls_back_to_explicit_names(self):
        session = str(uuid.uuid4())
        native = self.native_claude(session)
        self.add_peer("reviewer", "claude", session)
        self.add_peer("builder")
        cases = {"codex_thread_id_present": {**native, "CODEX_THREAD_ID": str(uuid.uuid4())},
                 "claude_session_variables_invalid": {**native, "CLAUDE_CODE_SESSION_ID": session.upper()},
                 "claude_session_variables_absent": {"CLAUDE_CODE_SESSION_ID": session}}
        for reason, env in cases.items():
            with self.subTest(reason=reason):
                self.assertEqual(reason, self.error("inbox", env=env)["native_session"]["reason"])
        # A command run through a nested client process does not inherit the name.
        shim = self.tmp / "codex-shim"
        shim.write_text('#!/bin/sh\n"$@"\nexit $?\n')
        shim.chmod(0o755)
        nested = self.finish(self.spawn(argv=[str(shim), *self.argv(["inbox"], True)], env=native), code=2)
        self.assertEqual("nested_client_process", nested["native_session"]["reason"])
        # A Claude process that is not an ancestor is not this command's session.
        sleeper = subprocess.Popen(["/bin/sleep", "60"])
        self.addCleanup(sleeper.wait)
        self.addCleanup(sleeper.kill)
        (self.home / (".claude/sessions/%d.json" % sleeper.pid)).write_text(json.dumps(
            {"pid": sleeper.pid, "sessionId": session, "procStart": process_start(sleeper.pid), "cwd": str(self.work)}))
        detached = {**native, "CLAUDE_PID": str(sleeper.pid)}
        self.assertEqual("claude_pid_not_an_ancestor", self.error("inbox", env=detached)["native_session"]["reason"])
        self.native_claude(session, procStart="1")
        self.assertEqual("claude_session_metadata_mismatch", self.error("inbox", env=native)["native_session"]["reason"])
        (self.home / (".claude/sessions/%d.json" % os.getpid())).unlink()
        self.assertEqual("claude_session_metadata_unavailable", self.error("inbox", env=native)["native_session"]["reason"])
        # Without recognized evidence an explicit, different Claude name works as before.
        self.assertEqual("option", self.htalk("--as", "builder", "inbox", env=native)["actor_source"])

    def test_claude_sessions_are_found_where_claude_config_dir_puts_them(self):
        session = str(uuid.uuid4())
        native = self.native_claude(session)
        self.add_peer("reviewer", "claude", session)
        _, listener = self.claude_recipient("bob")
        # A Claude Code started with CLAUDE_CONFIG_DIR keeps its session files there, not in the home directory.
        moved = self.tmp / "claude-config"
        (self.home / ".claude").rename(moved)
        elsewhere = {"CLAUDE_CONFIG_DIR": str(moved)}
        for name, there in (("without the variable", {}), ("with an empty one", {"CLAUDE_CONFIG_DIR": ""})):
            with self.subTest(name):
                self.error("peer", "check", "bob", env=there, error="file_not_found")
                unsent = self.htalk("--as", "reviewer", "send", "bob", "--message", "Q", env=there, code=2)
                self.assertEqual("not_submitted", unsent["submission"])
                self.assertEqual("claude_session_metadata_unavailable",
                                 self.error("inbox", env={**native, **there})["native_session"]["reason"])
        self.assertEqual(listener.path, self.htalk("peer", "check", "bob", env=elsewhere)["socket"])
        sent = self.htalk("send", "bob", "--message", "Q", env={**native, **elsewhere})
        self.assertEqual(("reviewer", "native_session", "submitted", "claude_socket_bytes_written"),
                         (sent["sender"], sent["actor_source"], sent["submission"], sent["notification_detail"]))
        self.assertEqual(1, len(listener.frames()))


class Notifications(HtalkCase):
    def test_claude_notice_is_one_frame_with_a_show_command_and_no_body(self):
        self.add_peer("alice")
        bob, listener = self.claude_recipient("bob")
        sent = self.htalk("--as", "alice", "send", "bob", "--message", "Private body 42")
        self.assertEqual(("submitted", "claude_socket_bytes_written", True),
                         (sent["submission"], sent["notification_detail"], sent["created"]))
        self.assertIsNotNone(self.kept(sent["id"])["notification_started_at"])
        self.assertIsNotNone(self.kept(sent["id"])["notification_finished_at"])
        frames = listener.frames()
        self.assertEqual(1, len(frames))
        frame = frames[0]
        self.assertEqual({"type", "session_id", "uuid", "msg_id", "from", "priority", "message"}, set(frame))
        self.assertEqual(("user", bob["session_id"], sent["id"], sent["id"], "htalk:alice", "next", "user"),
                         (frame["type"], frame["session_id"], frame["uuid"], frame["msg_id"], frame["from"],
                          frame["priority"], frame["message"]["role"]))
        content = frame["message"]["content"]
        self.assertNotIn("Private body 42", content)
        self.assertIn(sent["id"], content)
        show = next(line for line in content.splitlines() if line.startswith("htalk "))
        self.assertEqual(["htalk", "--db", str(self.db), "--as", "bob", "show", sent["id"]], words_of(show))
        shown = self.recover(show)
        self.assertEqual(("Private body 42", None), (shown["body"], shown["ack_at"]))
        checked = self.htalk("peer", "check", "bob")
        self.assertEqual({"harness": "claude", "session_id": bob["session_id"], "workspace": bob["workspace"],
                          "socket": listener.path, "retired_at": None}, checked)
        self.assertEqual([], listener.frames()[1:])

    def test_unconfirmed_notice_exits_2_saves_the_message_and_is_never_retried(self):
        self.add_peer("alice")
        self.add_peer("bob", "claude")
        chosen = str(uuid.uuid4())
        failed = self.htalk("--as", "alice", "send", "bob", "--id", chosen, "--message", "Q", code=2)
        self.assertEqual(("not_submitted", "recipient_unavailable", True),
                         (failed["submission"], failed["notification_detail"], failed["created"]))
        attempt = self.kept(chosen)
        self.assertIsNotNone(attempt["notification_started_at"])
        self.assertIn("Never repeat an uncertain notification", failed["next_action"])
        self.assertEqual(1, len(self.calls("claude", ["agents"])))
        retry = self.htalk("--as", "alice", "send", "bob", "--id", chosen, "--message", "Q")
        self.assertEqual((False, "not_submitted", attempt), (retry["created"], retry["submission"], self.kept(chosen)))
        for words in (["--as", "bob", "show", chosen], ["--as", "alice", "wait", chosen, "--seconds", "0"],
                      ["--as", "alice", "sent"], ["--as", "bob", "ack", chosen]):
            self.htalk(*words)
        answer = self.htalk("--as", "bob", "reply", chosen, "--message", "A", code=2)
        self.assertEqual("recipient_unavailable", answer["notification_detail"])
        self.assertEqual(answer["id"], self.htalk("--as", "bob", "reply", chosen, "--message", "A")["id"])
        self.assertEqual(2, len(self.calls("claude", ["agents"])))
        self.assertEqual({"status": "unsupported", "detail": "client_has_no_notification_removal"},
                         self.htalk("--as", "alice", "ack", answer["id"])["notification_cleanup"])
        self.assertEqual(2, len(self.calls("claude", ["agents"])))
        self.assertEqual("recipient_unavailable", self.error("peer", "check", "bob", error="recipient_unavailable")["error"])
        self.error("peer", "check", "zed", error="unknown_peer")


    def test_codex_acknowledgment_removes_only_its_confirmed_notice(self):
        self.add_peer("alice")
        bob = self.codex_recipient("bob")
        queue_id = str(uuid.uuid4())
        self.configure(codex_queue={"mode": "ok", "queue_id": queue_id})
        sent = self.htalk("--as", "alice", "send", "bob", "--message", "Q")
        self.htalk("--as", "bob", "show", sent["id"])
        self.htalk("--as", "bob", "inbox")
        self.assertEqual([], self.calls("codex", ["app-server"]))
        acked = self.htalk("--as", "bob", "ack", sent["id"])
        self.assertEqual({"status": "removed", "queue_id": queue_id}, acked["notification_cleanup"])
        self.assertEqual(["initialize", "initialized", "thread/queue/delete"], [c["rpc"] for c in self.calls("codex") if "rpc" in c])
        delete = [c for c in self.calls("codex") if c.get("rpc") == "thread/queue/delete"][0]
        self.assertEqual({"threadId": bob["session_id"], "queuedSubmissionId": queue_id}, delete["params"])
        self.configure(codex_app_server={"deleted": False})
        again = self.htalk("--as", "bob", "ack", sent["id"])
        self.assertEqual(({"status": "absent", "queue_id": queue_id}, acked["ack_at"]), (again["notification_cleanup"], again["ack_at"]))
        self.assertEqual(("submitted", "codex_cli_queued:" + queue_id), (again["submission"], again["notification_detail"]))
        self.assertEqual([sent["id"]], [m["id"] for m in self.htalk("--as", "bob", "inbox")["messages"]])
        quiet = self.htalk("--as", "alice", "send", "bob", "--message", "Polling only", "--no-notify")
        self.assertEqual({"status": "skipped", "detail": "no_confirmed_queue_receipt"},
                         self.htalk("--as", "bob", "ack", quiet["id"])["notification_cleanup"])
        # Unverifiable native access keeps the read mark and offers a retry.
        other = self.htalk("--as", "alice", "send", "bob", "--message", "Another")
        self.sql("DELETE FROM threads", path=self.codex_home / "state_5.sqlite")
        unavailable = self.htalk("--as", "bob", "ack", other["id"])
        self.assertEqual({"status": "unavailable", "detail": "recipient_not_in_codex_state"}, unavailable["notification_cleanup"])
        self.assertIsNotNone(unavailable["ack_at"])
        self.assertEqual(["htalk", "--db", str(self.db), "--as", "bob", "ack", other["id"]],
                         words_of(unavailable["recovery"]["retry_notification_cleanup"]))
        self.assertIn("retry_notification_cleanup", unavailable["next_action"])
        self.assertEqual(2, len([c for c in self.calls("codex") if c.get("rpc") == "thread/queue/delete"]))

    def test_opencode_prompt_is_posted_once_to_the_exact_session(self):
        server, url = self.opencode_server({"ses_muse": {"id": "ses_muse", "directory": str(self.work),
                                                         "time": {"created": 1, "updated": 2}}})
        self.add_peer("alice")
        self.add_peer("muse", "opencode", "ses_muse", None, "--url", url)
        checked = self.htalk("peer", "check", "muse")
        self.assertEqual(("opencode", "ses_muse", url, "1.18.30", "idle", "opencode_http_prompt_async", False, None),
                         (checked["harness"], checked["session_id"], checked["url"], checked["server_version"],
                          checked["runtime_status"], checked["transport"], checked["authenticated"], checked["retired_at"]))
        sent = self.htalk("--as", "alice", "send", "muse", "--message", "Private body 9")
        self.assertEqual(("submitted", "opencode_prompt_async_accepted"), (sent["submission"], sent["notification_detail"]))
        posts = [r for r in server.state["requests"] if r["method"] == "POST"]
        self.assertEqual(1, len(posts))
        self.assertEqual("/session/ses_muse/prompt_async", posts[0]["path"])
        text = posts[0]["body"]["parts"][0]["text"]
        self.assertEqual("text", posts[0]["body"]["parts"][0]["type"])
        self.assertIn(sent["id"], text)
        self.assertNotIn("Private body 9", text)
        self.assertIsNone(posts[0]["authorization"])
        self.assertFalse(self.htalk("--as", "alice", "send", "muse", "--id", sent["id"], "--message", "Private body 9")["created"])
        self.assertEqual(1, len([r for r in server.state["requests"] if r["method"] == "POST"]))
        # Nothing listens on a closed loopback port: rejected before transport.
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            closed = "http://127.0.0.1:%d" % probe.getsockname()[1]
        self.add_peer("gone", "opencode", "ses_gone", None, "--url", closed)
        refused = self.htalk("--as", "alice", "send", "gone", "--message", "Q", code=2)
        self.assertEqual(("not_submitted", "opencode_unreachable"), (refused["submission"], refused["notification_detail"]))


class OpenCodeServer(HtalkCase):
    """What htalk asks of an OpenCode server and what it makes of the answers. The fake server records
    every request; a test waits on that record, on the command's answer or on its exit."""

    def setUp(self):
        super().setUp()
        self.session = {"id": "ses_muse", "directory": str(self.work), "time": {"created": 1, "updated": 2}}
        self.server, self.url = self.opencode_server({"ses_muse": self.session})
        self.fake = self.server.state
        self.add_peer("alice")
        self.add_peer("muse", "opencode", "ses_muse", None, "--url", self.url)

    def posts(self):
        return [request for request in self.fake["requests"] if request["method"] == "POST"]

    def check(self, name="muse", env=None):
        return self.run_raw("peer", "check", name, env=env).json

    def send(self, *options, to="muse", env=None):
        sent = self.run_raw("--as", "alice", "send", to, "--message", "Q", *options, env=env).json
        return sent["submission"], sent["notification_detail"]

    def hold(self, path):
        """Make the server keep its answer to one path until the returned event is set."""
        started, release = threading.Event(), threading.Event()
        self.addCleanup(release.set)

        def hook(method, asked):
            if asked == path:
                started.set()
                release.wait(TIMEOUT)
        self.fake["hook"] = hook
        return started, release

    def test_check_asks_for_the_exact_session_and_reads_its_state(self):
        checked = self.htalk("peer", "check", "muse")
        self.assertEqual({"harness", "session_id", "workspace", "url", "server_version", "runtime_status", "transport", "authenticated"},
                         set(checked) - {"state", "name", "retired_at", "next_action", "recovery"})
        directory = urllib.parse.urlencode({"directory": str(self.work)})
        self.assertEqual([("GET", "/global/health", "", None), ("GET", "/session/ses_muse", directory, None),
                          ("GET", "/session/status", directory, None)],
                         [(r["method"], r["path"], r["query"], r["body"]) for r in self.fake["requests"]])
        link = self.tmp / "link"
        link.symlink_to(self.work)
        for status, session, key, expected in (
                ({"ses_muse": {"type": "busy"}}, {}, "runtime_status", "busy"),
                ({"ses_muse": None, "ses_other": {"type": "odd"}}, {}, "runtime_status", "idle"),
                ({"ses_muse": {"type": "unrecognized"}}, {}, "error", "opencode_invalid_response"),
                ([], {}, "error", "opencode_invalid_response"),
                ({}, {"directory": str(self.work / "elsewhere")}, "error", "recipient_identity_changed"),
                ({}, {"directory": ""}, "error", "opencode_invalid_response"),
                ({}, {"directory": str(link)}, "workspace", str(self.work)),
                ({}, {"time": {"updated": 2, "archived": 3}}, "error", "recipient_session_archived"),
                ({}, {"time": {"updated": 2, "archived": None}}, "runtime_status", "idle"),
                ({}, {"time": []}, "error", "opencode_invalid_response"),
                ({}, None, "error", "recipient_not_in_opencode_server")):
            with self.subTest(status=status, session=session):
                self.fake["status"] = status
                self.fake["sessions"] = {} if session is None else {"ses_muse": {**self.session, **session}}
                self.assertEqual(expected, self.check()[key])

    def test_an_answer_may_be_four_mebibytes_and_no_more(self):
        body = json.dumps({"healthy": True, "version": "large"}).encode()
        for size, key, expected in ((4 * 1024 * 1024, "server_version", "large"), (4 * 1024 * 1024 + 1, "error", "opencode_response_too_large")):
            answer = b"HTTP/1.1 200 OK\r\nContent-Length: %d\r\n\r\n" % size + body.ljust(size)
            self.fake["hook"] = lambda method, path, answer=answer: answer if path == "/global/health" else None
            self.assertEqual(expected, self.check()[key])

    def test_the_status_of_the_post_decides_what_is_recorded(self):
        for status, expected in ((204, ("submitted", "opencode_prompt_async_accepted")),
                                 (404, ("not_submitted", "recipient_not_in_opencode_server")),
                                 (401, ("not_submitted", "opencode_unauthorized")),
                                 (409, ("not_submitted", "opencode_http_409")), (400, ("not_submitted", "opencode_http_400")),
                                 (500, ("submission_unknown", "opencode_http_500")), (200, ("submission_unknown", "opencode_http_200")),
                                 (302, ("submission_unknown", "opencode_http_302"))):
            with self.subTest(status=status):
                self.fake["prompt"] = status
                self.assertEqual(expected, self.send())
        self.assertEqual(8, len(self.posts()))

    def test_an_answer_that_is_not_whole_leaves_the_notice_unknown(self):
        whole = b"HTTP/1.1 400 Bad\r\nContent-Length: 3\r\n\r\n{x}"
        cases = ((whole, ("not_submitted", "opencode_http_400")),
                 (b"", ("submission_unknown", "opencode_connection_closed")),
                 (b"garbage\r\n\r\n", ("submission_unknown", "opencode_invalid_response")),
                 (b"HTTP/1.1 400 Bad\r\nContent-Length: 1000\r\n\r\n{}", ("submission_unknown", "opencode_connection_closed")))
        for count, (answer, expected) in enumerate(cases, 1):
            with self.subTest(answer=answer):
                self.fake["hook"] = lambda method, path, answer=answer: answer if method == "POST" else None
                chosen = str(uuid.uuid4())
                self.assertEqual(expected, self.send("--id", chosen))
                # The same send again finds the saved one, and the prompt is not posted a second time.
                self.assertEqual(expected, self.send("--id", chosen))
                self.assertEqual(count, len(self.posts()))

    def test_a_server_that_takes_the_post_and_never_answers_leaves_the_notice_unknown(self):
        started, release = self.hold("/session/ses_muse/prompt_async")
        self.assertEqual(("submission_unknown", "opencode_timed_out"), self.send())
        self.assertTrue(started.is_set())
        self.assertEqual(1, len(self.posts()))

    def test_a_failed_check_before_the_post_sends_nothing(self):
        self.fake["sessions"] = {}
        self.assertEqual(("not_submitted", "recipient_not_in_opencode_server"), self.send())
        self.fake["sessions"] = {"ses_muse": {**self.session, "directory": str(self.tmp)}}
        self.assertEqual(("not_submitted", "recipient_identity_changed"), self.send())
        self.fake["sessions"] = {"ses_muse": self.session}
        self.fake["hook"] = lambda method, path: b"" if path == "/global/health" else None
        self.assertEqual(("not_submitted", "opencode_connection_closed"), self.send())
        self.assertEqual([], self.posts())

    def test_an_acknowledgment_during_the_checks_prevents_the_post(self):
        started, release = self.hold("/session/status")
        chosen = str(uuid.uuid4())
        sending = self.spawn("--as", "alice", "send", "muse", "--id", chosen, "--message", "Q")
        self.assertTrue(started.wait(TIMEOUT))
        self.htalk("--as", "muse", "ack", chosen)
        release.set()
        result = self.finish(sending, code=0)
        self.assertEqual(("not_submitted", "acknowledged_before_notification"), (result["submission"], result["notification_detail"]))
        self.assertEqual([], self.posts())

    def test_an_interrupt_during_the_checks_posts_nothing(self):
        started, release = self.hold("/session/status")
        chosen = str(uuid.uuid4())
        sending = self.spawn("--as", "alice", "send", "muse", "--id", chosen, "--message", "Q")
        self.assertTrue(started.wait(TIMEOUT))
        sending.send_signal(INTERRUPT)
        # The command answers while the server still holds its last check.
        result = self.finish(sending, code=130)
        self.assertEqual(("interrupted", chosen), (result["state"], result["message_id"]))
        release.set()
        del self.fake["hook"]
        again = self.run_raw("--as", "alice", "send", "muse", "--id", chosen, "--message", "Q").json
        self.assertEqual((False, "submission_unknown"), (again["created"], again["submission"]))
        self.assertEqual(3, len(self.fake["requests"]))
        self.assertEqual([], self.posts())

    def test_credentials_come_from_the_environment_and_a_proxy_is_never_used(self):
        proxy = socket.socket()
        self.addCleanup(proxy.close)
        proxy.bind(("127.0.0.1", 0))
        proxy.listen(8)
        proxy.setblocking(False)
        env = dict.fromkeys(("HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"),
                            "http://127.0.0.1:%d" % proxy.getsockname()[1])
        self.assertIs(False, self.check(env=env)["authenticated"])
        self.assertEqual({None}, {request["authorization"] for request in self.fake["requests"]})

        def basic(user):
            return "Basic " + base64.b64encode(("%s:invented-secret" % user).encode()).decode()
        self.fake["authorization"] = basic("opencode")
        self.assertEqual("opencode_unauthorized", self.check(env=env)["error"])
        found = self.run_raw("peer", "discover", "--harness", "opencode", "--opencode-url", self.url, env=env).json
        self.assertEqual("opencode_unauthorized", found["sources"][0]["error"])
        secret = {**env, "OPENCODE_SERVER_PASSWORD": "invented-secret", "OPENCODE_SERVER_USERNAME": ""}
        checked = self.check(env=secret)
        self.assertIs(True, checked["authenticated"])
        self.assertNotIn("invented-secret", json.dumps(checked))
        self.assertEqual(("submitted", "opencode_prompt_async_accepted"), self.send(env=secret))
        self.assertEqual(basic("opencode"), self.posts()[0]["authorization"])
        for change, header in (({"OPENCODE_SERVER_USERNAME": "other"}, basic("other")), ({"OPENCODE_SERVER_PASSWORD": ""}, None)):
            self.assertEqual("opencode_unauthorized", self.check(env={**secret, **change})["error"])
            self.assertEqual(header, self.fake["requests"][-1]["authorization"])
        asked = len(self.fake["requests"])
        self.assertEqual("invalid_opencode_credentials", self.check(env={**env, "OPENCODE_SERVER_PASSWORD": "\udcff"})["error"])
        self.assertEqual(asked, len(self.fake["requests"]))
        with self.assertRaises(BlockingIOError):
            proxy.accept()

    def test_https_is_used_only_with_a_certificate_that_is_trusted(self):
        sessions = {name: {**self.session, "id": name} for name in ("ses_number", "ses_name")}
        server, url = self.opencode_server(sessions, tls=True)
        (self.tmp / "ca.pem").write_text(OPENCODE_CA)
        trusted, untrusted = {"SSL_CERT_FILE": str(self.tmp / "ca.pem")}, {"SSL_CERT_FILE": str(self.tmp / "none.pem")}
        self.add_peer("number", "opencode", "ses_number", None, "--url", url)
        self.add_peer("name", "opencode", "ses_name", None, "--url", url.replace("127.0.0.1", "localhost"))
        for name in ("number", "name"):
            self.assertEqual("1.18.30", self.check(name, env=trusted)["server_version"])
            self.assertEqual(("submitted", "opencode_prompt_async_accepted"), self.send(to=name, env=trusted))
        asked = len(server.state["requests"])
        self.assertEqual(2, len([request for request in server.state["requests"] if request["method"] == "POST"]))
        for name in ("number", "name"):
            self.assertEqual("opencode_unreachable", self.check(name, env=untrusted)["error"])
            self.assertEqual(("not_submitted", "opencode_unreachable"), self.send(to=name, env=untrusted))
        self.assertEqual(asked, len(server.state["requests"]))


class NotificationOrdering(HtalkCase):
    """Late acknowledgment, retirement and the requester's own wait at the final check."""

    def reset_gate(self, name):
        for suffix in (".started", ".release"):
            (self.state / (name + suffix)).unlink(missing_ok=True)

    def test_state_changed_during_preflight_prevents_the_frame(self):
        self.add_peer("alice")
        _, listener = self.claude_recipient("bob")
        self.configure(claude_agents={"rows": self.claude_rows, "gate": True})
        for change, detail in ((["--as", "bob", "ack"], "acknowledged_before_notification"),
                               (["peer", "retire", "bob"], "recipient_retired")):
            with self.subTest(detail=detail):
                self.reset_gate("claude_agents")
                chosen = str(uuid.uuid4())
                sending = self.spawn("--as", "alice", "send", "bob", "--id", chosen, "--message", "Q")
                self.started("claude_agents")
                self.htalk(*change, *([chosen] if change[-1] == "ack" else []))
                self.release("claude_agents")
                result = self.finish(sending, code=0)
                self.assertEqual(("not_submitted", detail), (result["submission"], result["notification_detail"]))
                self.assertIsNotNone(self.kept(result["id"])["notification_finished_at"])
                self.assertEqual([], listener.frames())
                self.htalk("peer", "restore", "bob")
        # Acknowledgment does not answer the question.
        self.assertEqual(2, self.htalk("--as", "bob", "inbox")["total"])

    def test_answer_returned_by_the_requester_wait_is_not_notified(self):
        self.add_peer("alice", "claude")
        self.add_peer("bob")
        question = self.htalk("--as", "alice", "send", "bob", "--message", "Q", "--no-notify")["id"]
        waiting = self.spawn("--as", "alice", "wait", question, "--seconds", "20")
        wait_for(self.waits, message="the wait registration")
        self.assertEqual([(question, "alice")], self.waits())
        # Without that receipt this notice would fail with recipient_unavailable and exit 2.
        answer = self.htalk("--as", "bob", "reply", question, "--message", "Answer")
        self.assertEqual(("not_submitted", "returned_by_recipient_wait", None),
                         (answer["submission"], answer["notification_detail"], answer["ack_at"]))
        self.assertIsNotNone(self.kept(answer["id"])["wait_returned_at"])
        self.assertEqual(set(), UNSETTLED & set(answer))
        received = self.finish(waiting)
        self.assertEqual((answer["id"], "Answer"), (received["reply"]["id"], received["reply"]["body"]))
        self.assertIsNotNone(self.kept(received["reply"]["id"])["wait_returned_at"])
        self.assertEqual(set(), UNSETTLED & set(received))
        # Registration cleanup is best effort; the reply writer may retain it.
        self.assertEqual([answer["id"]], [m["id"] for m in self.htalk("--as", "alice", "inbox")["messages"]])

    def test_send_wait_that_returns_an_answer_exits_zero_despite_an_unconfirmed_notice(self):
        self.add_peer("alice")
        self.add_peer("bob")
        chosen = str(uuid.uuid4())
        sending = self.spawn("--as", "alice", "send", "bob", "--id", chosen, "--message", "Q", "--wait", "20")
        wait_for(self.waits, message="the send --wait registration")
        self.htalk("--as", "bob", "reply", chosen, "--message", "Answer", "--no-notify")
        result = self.finish(sending, code=0)
        self.assertEqual((chosen, True, "not_submitted", any_peer("recipient_unavailable", "pull_only"), "Answer"),
                         (result["id"], result["created"], result["submission"], result["notification_detail"],
                          result["reply"]["body"]))
        self.assertIsNotNone(self.kept(result["reply"]["id"])["wait_returned_at"])
        self.assertEqual("option", result["actor_source"])


class Interrupts(HtalkCase):
    def interrupt(self, process, code=130):
        process.send_signal(INTERRUPT)
        result = self.finish(process, code=code)
        self.assertEqual("interrupted", result["state"])
        self.assertIn("Do not resend", result["next_action"])
        return result

    def test_interrupted_notification_is_saved_uncertain_and_not_replayed(self):
        self.add_peer("alice")
        self.codex_recipient("bob")
        self.configure(codex_queue={"mode": "block"})
        chosen = str(uuid.uuid4())
        sending = self.spawn("--as", "alice", "send", "bob", "--id", chosen.upper(), "--message", "Q")
        self.started("codex_queue")
        result = self.interrupt(sending)
        self.assertEqual((chosen, "saved", "option"), (result["message_id"], result["persistence"], result["actor_source"]))
        self.assertEqual({"peers", "sent", "inbox", "show"}, set(result["recovery"]))
        shown = self.recover(result["recovery"]["show"])
        self.assertEqual(("submission_unknown", None), (shown["submission"], self.kept(shown["id"])["notification_finished_at"]))
        self.assertIsNotNone(self.kept(shown["id"])["notification_started_at"])
        self.assertEqual([chosen], [m["id"] for m in self.recover(result["recovery"]["sent"])["messages"]])
        self.configure(codex_queue={"mode": "ok", "queue_id": str(uuid.uuid4())})
        retry = self.htalk("--as", "alice", "send", "bob", "--id", chosen, "--message", "Q")
        self.assertEqual((False, "submission_unknown"), (retry["created"], retry["submission"]))
        self.assertEqual(1, len(self.calls("codex", ["queue"])))
        self.assertEqual([chosen], [m["id"] for m in self.htalk("--as", "bob", "inbox")["messages"]])

    def test_an_interrupted_client_command_ends_with_what_it_started(self):
        self.add_peer("alice")
        self.codex_recipient("bob")
        # The client has started a process that ignores a request to end and keeps the client's output open.
        self.configure(codex_queue={"mode": "block", "descendant": True})
        sending = self.spawn("--as", "alice", "send", "bob", "--message", "Q")
        self.started("codex_queue")
        (started,) = [call["pid"] for call in self.calls("codex") if call.get("descendant")]
        self.assertFalse(gone(started))
        self.assertEqual("saved", self.interrupt(sending)["persistence"])
        wait_for(lambda: gone(started), message="what the client command started to end")

    def test_interrupted_wait_forgets_its_registration(self):
        self.add_peer("alice")
        self.add_peer("bob")
        question = self.htalk("--as", "alice", "send", "bob", "--message", "Q", "--no-notify")["id"]
        waiting = self.spawn("--as", "alice", "wait", question, "--seconds", "30")
        wait_for(self.waits, message="the wait registration")
        result = self.interrupt(waiting)
        self.assertEqual((question, "unknown"), (result["message_id"], result["persistence"]))
        self.assertEqual({"peers", "sent", "inbox", "show"}, set(result["recovery"]))
        self.assertEqual([], self.waits())
        self.assertEqual("saved", self.recover(result["recovery"]["show"])["state"])

    def test_interrupted_ack_keeps_the_read_mark_and_offers_cleanup_retry(self):
        self.add_peer("alice")
        self.codex_recipient("bob")
        queue_id = str(uuid.uuid4())
        self.configure(codex_queue={"mode": "ok", "queue_id": queue_id}, codex_app_server={"mode": "block"})
        question = self.htalk("--as", "alice", "send", "bob", "--message", "Q")["id"]
        acking = self.spawn("--as", "bob", "ack", question)
        self.started("codex_delete")
        result = self.interrupt(acking)
        self.assertEqual(question, result["message_id"])
        self.assertEqual(["htalk", "--db", str(self.db), "--as", "bob", "ack", question],
                         words_of(result["recovery"]["retry_notification_cleanup"]))
        saved = self.htalk("--as", "bob", "show", question)
        self.assertIsNotNone(saved["ack_at"])
        self.configure(codex_app_server={})
        recovered = self.recover(result["recovery"]["retry_notification_cleanup"])
        self.assertEqual({"status": "removed", "queue_id": queue_id}, recovered["notification_cleanup"])
        self.assertEqual((saved["ack_at"], "submitted"), (recovered["ack_at"], recovered["submission"]))
        self.assertEqual(1, len(self.calls("codex", ["queue"])))

    def test_interrupted_ack_returns_while_the_client_still_holds_the_removal(self):
        self.add_peer("alice")
        self.codex_recipient("bob")
        self.configure(codex_queue={"mode": "ok", "queue_id": str(uuid.uuid4())}, codex_app_server={"mode": "block"})
        first, second = (self.htalk("--as", "alice", "send", "bob", "--message", text)["id"] for text in ("One", "Two"))

        def removals():
            return [call for call in self.calls("codex") if call.get("rpc") == "thread/queue/delete"]

        # Two acknowledgments wait on the same held client. Nobody interrupts the first one.
        waiting = self.spawn("--as", "bob", "ack", first)
        wait_for(lambda: len(removals()) == 1, message="the first removal to reach the client")
        acking = self.spawn("--as", "bob", "ack", second)
        wait_for(lambda: len(removals()) == 2, message="the second removal to reach the client")
        self.assertEqual(second, self.interrupt(acking)["message_id"])
        self.assertIsNone(waiting.poll(), "the interrupted ack waited as long as the one nobody interrupted")
        self.release("codex_delete")
        self.assertIsNotNone(self.finish(waiting)["ack_at"])


class ProcessRecovery(HtalkCase):
    def setUp(self):
        super().setUp()
        self.alice = self.codex_recipient("alice")
        self.bob = self.codex_recipient("bob")
        self.queue_id = str(uuid.uuid4())
        self.configure(codex_queue={"queue_id": self.queue_id})

    def kill(self, process):
        process.kill()
        process.communicate(timeout=10)
        self.assertEqual(-signal.SIGKILL, process.returncode)

    def send_words(self, chosen, body="Question?"):
        return ("--as", "alice", "send", "bob", "--id", chosen, "--message", body)

    def two_retries(self, chosen):
        processes = [self.spawn(*self.send_words(chosen)) for _ in range(2)]
        results = [self.finish(process) for process in processes]
        self.assertEqual([False, False], [result["created"] for result in results])
        self.assertEqual([chosen, chosen], [result["id"] for result in results])
        return results

    def test_sigkill_during_notification_preserves_unknown_and_polling_recovery(self):
        self.configure(codex_queue={"mode": "block"})
        chosen = str(uuid.uuid4())
        sending = self.spawn(*self.send_words(chosen))
        self.started("codex_queue")
        before = self.htalk("--as", "alice", "show", chosen)
        self.assertEqual(("submission_unknown", None),
                         (before["submission"], self.kept(before["id"])["notification_finished_at"]))
        self.assertIsNotNone(self.kept(before["id"])["notification_started_at"])
        self.kill(sending)

        for retry in self.two_retries(chosen):
            self.assertEqual(("Question?", before["created_at"], "submission_unknown", None),
                             (retry["body"], retry["created_at"], retry["submission"],
                              self.kept(retry["id"])["notification_finished_at"]))
        self.assertEqual([(1,)], self.sql("SELECT COUNT(*) FROM messages WHERE id=?", (chosen,)))
        self.assertEqual([chosen], [m["id"] for m in self.htalk("--as", "bob", "inbox")["messages"]])
        acknowledged = self.htalk("--as", "bob", "ack", chosen)
        self.assertIsNotNone(acknowledged["ack_at"])
        self.assertEqual(("submission_unknown", None, "pending"),
                         (acknowledged["submission"], self.kept(acknowledged["id"])["notification_finished_at"],
                          acknowledged["notification_cleanup"]["status"]))
        repeated_ack = self.recover(acknowledged["recovery"]["retry_notification_cleanup"])
        self.assertEqual((acknowledged["ack_at"], "pending"),
                         (repeated_ack["ack_at"], repeated_ack["notification_cleanup"]["status"]))
        # This gated fake exits unsuccessfully without a receipt when released.
        # Observe the late failure, without claiming to simulate late acceptance.
        child_pid = self.calls("codex", ["queue"])[0]["pid"]
        self.assertTrue(self.runs_fake(child_pid), "the blocked fake must outlive the killed sender")
        self.release("codex_queue")
        wait_for(lambda: not self.runs_fake(child_pid), message="the orphaned fake client to exit")
        after_child = self.htalk("--as", "bob", "show", chosen)
        self.assertEqual(("submission_unknown", None, acknowledged["ack_at"]),
                         (after_child["submission"], self.kept(after_child["id"])["notification_finished_at"],
                          after_child["ack_at"]))
        after_child_ack = self.recover(acknowledged["recovery"]["retry_notification_cleanup"])
        self.assertEqual((acknowledged["ack_at"], "pending"),
                         (after_child_ack["ack_at"], after_child_ack["notification_cleanup"]["status"]))
        self.assertEqual([], self.calls("codex", ["app-server"]))
        self.assertEqual(1, len(self.calls("codex", ["queue"])))

    def test_sigkill_after_receipt_keeps_submission_and_allows_a_notified_answer(self):
        chosen = str(uuid.uuid4())
        sending = self.spawn(*self.send_words(chosen), "--wait", "30")
        wait_for(self.waits, message="the wait after a confirmed notification")
        before = self.htalk("--as", "alice", "show", chosen)
        self.assertEqual("submitted", before["submission"])
        attempt = self.kept(chosen)
        self.assertIsNotNone(attempt["notification_finished_at"])
        self.kill(sending)
        self.assertTrue(self.waits(), "SIGKILL must leave the registered poll behind")

        for retry in self.two_retries(chosen):
            self.assertEqual(("submitted", before["created_at"], attempt),
                             (retry["submission"], retry["created_at"], self.kept(chosen)))
        answer = self.htalk("--as", "bob", "reply", chosen, "--message", "Answer")
        self.assertEqual("submitted", answer["submission"])
        self.assertIsNotNone(self.kept(answer["id"])["notification_finished_at"])
        recovered = self.htalk("--as", "alice", "wait", chosen, "--seconds", "0")
        self.assertEqual(answer["id"], recovered["reply"]["id"])
        self.assertEqual("submitted", recovered["submission"])
        notices = self.calls("codex", ["queue"])
        self.assertEqual([self.bob["session_id"], self.alice["session_id"]],
                         [call["argv"][2] for call in notices])


    def test_concurrent_processes_save_one_request_and_refuse_conflicts(self):
        for conflicting in (False, True):
            with self.subTest(conflicting=conflicting):
                for suffix in ("started", "release"):
                    (self.state / ("codex_queue." + suffix)).unlink(missing_ok=True)
                self.configure(codex_queue={"mode": "block"})
                chosen = str(uuid.uuid4())
                bodies = ["Changed?" if conflicting and i % 2 else "Question?" for i in range(32)]
                before_calls = len(self.calls("codex", ["queue"]))
                # Launch every sender before collecting any result. The winning
                # client's gate keeps its notification unfinished during the race.
                processes = [self.spawn(*self.send_words(chosen, body)) for body in bodies]
                self.started("codex_queue")
                wait_for(lambda: sum(process.poll() is not None for process in processes) == 31,
                         message="all other senders to finish before releasing the notification")
                unfinished = self.htalk("--as", "alice", "show", chosen)
                self.assertIsNone(self.kept(unfinished["id"])["notification_finished_at"])
                self.release("codex_queue")
                outputs = []
                for process in processes:
                    stdout, stderr = process.communicate(timeout=30)
                    self.assertIn(process.returncode, (0, 2), stdout + stderr)
                    outputs.append((process.returncode, json.loads(stdout)))
                saved = self.htalk("--as", "alice", "show", chosen)
                created = [result for _, result in outputs if result.get("created") is True]
                self.assertEqual(1, len(created))
                self.assertEqual([(1,)], self.sql("SELECT COUNT(*) FROM messages WHERE id=?", (chosen,)))
                self.assertEqual(before_calls + 1, len(self.calls("codex", ["queue"])))
                self.assertEqual(("submission_unknown", created[0]["created_at"]),
                                 (saved["submission"], saved["created_at"]))
                for body, (code, result) in zip(bodies, outputs):
                    if body != saved["body"]:
                        self.assertEqual((2, "message_id_conflict"), (code, result["error"]))
                    else:
                        self.assertEqual((chosen, body, saved["created_at"]),
                                         (result["id"], result["body"], result["created_at"]))
                        self.assertEqual(2 if result["created"] else 0, code)
                        if not result["created"]:
                            # A retry can observe the saved row before the winner
                            # claims its notification, as well as after that claim.
                            self.assertIn(result["submission"], ("not_submitted", "submission_unknown"))

    def test_lookup_retry_and_ack_can_race_an_unfinished_notification(self):
        self.configure(codex_queue={"mode": "block"})
        chosen = str(uuid.uuid4())
        sending = self.spawn(*self.send_words(chosen))
        self.started("codex_queue")
        lookup = self.spawn("--as", "alice", "show", chosen)
        retries = [self.spawn(*self.send_words(chosen)) for _ in range(8)]
        acking = self.spawn("--as", "bob", "ack", chosen)
        self.assertEqual(chosen, self.finish(lookup)["id"])
        for process in retries:
            result = self.finish(process)
            self.assertEqual((chosen, False, "submission_unknown"),
                             (result["id"], result["created"], result["submission"]))
        acknowledged = self.finish(acking)
        self.assertEqual("pending", acknowledged["notification_cleanup"]["status"])
        self.assertIsNotNone(acknowledged["ack_at"])
        self.release("codex_queue")
        # The saved ack makes this invocation successful even though the client
        # never supplies a confirmed queue receipt.
        final = self.finish(sending)
        self.assertEqual(("submission_unknown", acknowledged["ack_at"]),
                         (final["submission"], final["ack_at"]))
        self.assertEqual(1, len(self.calls("codex", ["queue"])))


class Discovery(HtalkCase):
    def test_native_metadata_reads_see_uncheckpointed_python_wal(self):
        peer = self.codex_recipient("wal_peer")
        codex_path = self.codex_home / "state_5.sqlite"
        with closing(sqlite3.connect(codex_path)) as writer:
            self.assertEqual("wal", writer.execute("PRAGMA journal_mode=WAL").fetchone()[0])
            writer.execute("PRAGMA wal_autocheckpoint=0")
            writer.execute("UPDATE threads SET archived=1 WHERE id=?", (peer["session_id"],))
            writer.commit()
            self.assertGreater(Path(str(codex_path) + "-wal").stat().st_size, 0)
            self.error("peer", "check", "wal_peer", error="recipient_is_not_an_unarchived_codex_cli_session")
            writer.execute("UPDATE threads SET archived=0 WHERE id=?", (peer["session_id"],))
            writer.commit()
            self.assertEqual(peer["session_id"], self.htalk("peer", "check", "wal_peer")["session_id"])
        opencode_path = self.home / ".local/share/opencode/opencode.db"
        opencode_path.parent.mkdir(parents=True, exist_ok=True)
        with closing(sqlite3.connect(opencode_path)) as writer:
            self.assertEqual("wal", writer.execute("PRAGMA journal_mode=WAL").fetchone()[0])
            writer.execute("PRAGMA wal_autocheckpoint=0")
            writer.execute("CREATE TABLE session (id TEXT, parent_id TEXT, directory TEXT, time_updated INTEGER, time_archived INTEGER)")
            writer.execute("INSERT INTO session VALUES (?, NULL, ?, 2000, NULL)", ("ses_wal", str(self.work)))
            writer.commit()
            self.assertGreater(Path(str(opencode_path) + "-wal").stat().st_size, 0)
            result = self.htalk("peer", "discover", "--harness", "opencode", "--opencode-url", "http://127.0.0.1:abc")
            self.assertEqual(["ses_wal"], [row["session_id"] for row in result["sessions"]])
            self.assertEqual(2, result["sessions"][0]["updated_at"])


    def test_claude_discovery_is_read_only_and_reports_coverage(self):
        self.need_native()
        from compat_support import ClaudeSocket
        live = ClaudeSocket(self.tmp / "live.sock")
        self.sockets.append(live)
        good, stale = str(uuid.uuid4()), str(uuid.uuid4())
        (self.home / ".claude/sessions/5001.json").write_text(json.dumps(
            {"pid": 5001, "sessionId": good, "cwd": str(self.work), "messagingSocketPath": live.path}))
        self.configure(claude_agents={"rows": [{"sessionId": good, "cwd": str(self.work), "pid": 5001},
                                               {"sessionId": stale, "cwd": str(self.work), "pid": 5002}]})
        found = self.htalk("peer", "discover", "--harness", "claude")
        self.assertEqual([{"harness": "claude", "session_id": good, "workspace": str(self.work),
                           "runtime_status": "running", "source": "claude_agents", "pid": 5001}], found["sessions"])
        self.assertEqual([("claude", "claude_agents", "partial", 1)],
                         [(s["harness"], s["source"], s["status"], s.get("rejected")) for s in found["sources"]])
        self.assertEqual({"sessions", "sources", "scope"}, set(found))
        self.assertEqual([], self.htalk("peer", "discover", "--harness", "claude", "--workspace", str(self.tmp))["sessions"])
        self.assertEqual([], live.frames())
        self.assertFalse(self.db.parent.exists())
        self.configure(claude_agents={"exit": 1})
        failed = self.htalk("peer", "discover", "--harness", "claude", code=2)
        self.assertEqual(([], "unavailable"), (failed["sessions"], failed["sources"][0]["status"]))
        for words, error in ((["--harness", "claude", "--codex-socket", str(self.tmp / "x.sock")], "codex_socket_requires_codex_discovery"),
                             (["--harness", "codex", "--opencode-url", "http://127.0.0.1:1"], "opencode_url_requires_opencode_discovery")):
            with self.subTest(error=error):
                self.error("peer", "discover", *words, error=error)
        self.assertFalse(self.db.parent.exists())

    def test_codex_writer_locks_are_not_read_where_the_system_keeps_no_table_of_them(self):
        self.need_native()
        if LOCK_TABLE:
            self.skipTest("this system keeps a table of held locks, and discovery reads it")
        # Nothing answers on the default socket either, so no source could be asked.
        found = self.htalk("peer", "discover", "--harness", "codex", code=2)
        self.assertEqual([], found["sessions"])
        self.assertEqual([("unavailable", "unsupported_on_this_platform")],
                         [(s["status"], s["detail"]) for s in found["sources"] if s["source"] == "codex_writer_locks"])

    def test_a_failure_below_a_client_is_a_fixed_code(self):
        self.need_native()

        def source(*words):
            first = self.htalk("peer", "discover", *words, code=2)["sources"][0]
            return first.get("error") or first["detail"]
        send = lambda name: self.htalk("--as", "alice", "send", name, "--message", "Q", code=2)["notification_detail"]
        check = lambda name: self.htalk("peer", "check", name, code=2)["error"]
        self.htalk("peer", "add", "alice", "--harness", "generic", "--delivery", "pull")
        # `claude agents --json` that fails, that is not JSON, that is not a list.
        for settings, code in (({"exit": 1}, "command_failed"), ({"stdout": "not json"}, "invalid_json"),
                               ({"stdout": "{}"}, "invalid_claude_agents_response")):
            self.configure(claude_agents=settings)
            self.assertEqual(code, source("--harness", "claude"))
        # A Claude socket nobody listens on, then no socket; then a session file that cannot be read.
        self.configure(claude_agents={})
        _, listener = self.claude_recipient("cl")
        saved = next((self.home / ".claude/sessions").glob("*.json"))
        listener.close()
        self.assertEqual("connection_refused", send("cl"))
        os.unlink(listener.path)
        self.assertEqual("file_not_found", send("cl"))
        for content, code in ((b"not json", "invalid_json"), (b"[1]", "invalid_client_data"), (b"\xff\xfe", "invalid_utf8")):
            saved.write_bytes(content)
            self.assertEqual(code, check("cl"))
        # Codex: no socket and a dead one; saved state that is missing, then not a database; a config that is not one.
        dead = self.tmp / "dead.sock"
        with socket.socket(socket.AF_UNIX) as unused:
            unused.bind(str(dead))
        self.assertEqual("file_not_found", source("--harness", "codex", "--codex-socket", self.tmp / "none.sock"))
        self.assertEqual("connection_refused", source("--harness", "codex", "--codex-socket", dead))
        self.add_peer("cx", "codex")
        self.assertEqual(("client_database_unavailable",) * 2, (check("cx"), send("cx")))
        (self.codex_home / "state_5.sqlite").write_bytes(b"not a database, " * 64)
        self.assertEqual("client_database_corrupt", check("cx"))
        (self.codex_home / "state_5.sqlite").unlink()
        (self.codex_home / "config.toml").write_text("sqlite_home = 5\n")
        self.assertEqual("invalid_codex_config", check("cx"))
        # An OpenCode server that answers with something that is not HTTP, and one that hangs up.
        for name, answer, code in (("oa", b"garbage\r\n\r\n", "opencode_invalid_response"), ("ob", None, "opencode_connection_closed")):
            server = socket.socket()
            self.addCleanup(server.close)
            server.bind(("127.0.0.1", 0))
            server.listen(8)

            def serve(server=server, answer=answer):
                while True:
                    try:
                        connection, _ = server.accept()
                    except OSError:
                        return
                    with connection:
                        if answer:
                            connection.recv(65536)
                            connection.sendall(answer)
            threading.Thread(target=serve, daemon=True).start()
            url = "http://127.0.0.1:%d" % server.getsockname()[1]
            self.add_peer(name, "opencode", None, None, "--url", url)
            self.assertEqual((code,) * 3, (check(name), send(name), source("--harness", "opencode", "--opencode-url", url)))


class OpenCodeDiscovery(HtalkCase):
    """Read-only OpenCode discovery: server sources, saved metadata and per-record diagnostics."""
    NO_SERVER = "http://127.0.0.1:abc"  # Malformed, so nothing is asked and the default server is not tried.
    UPDATED = 1788990000000

    def database(self, data):
        return (data or self.home / ".local/share") / "opencode/opencode.db"

    def saved(self, name, rows, *changes):
        """A data home with saved rows of id, parent_id, directory, time_updated, time_archived."""
        path = self.database(self.tmp / name)
        path.parent.mkdir(parents=True)
        with closing(sqlite3.connect(path)) as db, db:
            db.execute("CREATE TABLE session (id, parent_id TEXT, directory, time_updated, time_archived INTEGER, title TEXT)")
            db.executemany("INSERT INTO session VALUES (?, ?, ?, ?, ?, 'invented title')", rows)
            for change in changes:
                db.execute("UPDATE session SET " + change)
        return self.tmp / name

    def saved_source(self, data, status="ok", error=None, detail=None, **rejected):
        return {"harness": "opencode", "source": "opencode_saved", "path": str(self.database(data)), "status": status,
                "error": error, "detail": detail, **rejected}

    def discover(self, data, *urls, workspace=None):
        """Sessions and sources. The saved file stays byte for byte, and only a run with no usable source fails."""
        path = self.database(data)
        before = path.read_bytes() if path.is_file() else None
        words = [word for url in urls or [self.NO_SERVER] for word in ("--opencode-url", url)]
        words += ["--workspace", workspace] if workspace else []
        result = self.run_raw("peer", "discover", "--harness", "opencode", *words, env={"XDG_DATA_HOME": str(data)})
        self.assertIsInstance(result.json, dict, result.stdout + result.stderr)
        sources = result.json["sources"]
        self.assertEqual(2 if all(s["status"] == "unavailable" for s in sources) else 0, result.code, result.stdout)
        self.assertEqual(before, path.read_bytes() if path.is_file() else None)
        self.assertNotIn("invented title", result.stdout)
        return result.json["sessions"], sources

    def listed(self, session_id, directory, **changes):
        session = {"id": session_id, "slug": "invented-slug", "projectID": "prj_invented", "directory": directory,
                   "title": "invented title", "version": "1.18.30", "time": {"created": 1000000, "updated": self.UPDATED}}
        return {**session, **changes}

    def test_malformed_server_records_preserve_valid_sessions_before_and_after(self):
        ws = str(self.work)
        server, url = self.opencode_server({})
        server.state["listed"] = [
            self.listed("ses_first", ws), self.listed("bad_id", ws), self.listed("ses_bad_directory", None),
            {"id": "ses_bad_time", "directory": ws, "time": []}, self.listed("ses_bad_status", ws), "not an object",
            self.listed("ses_last", ws)]
        server.state["status"] = {"ses_bad_status": {"type": "unrecognized"}, "ses_last": {"type": "retry"}}
        sessions, sources = self.discover(self.tmp / "absent", url)
        self.assertEqual(["ses_first", "ses_last"], [row["session_id"] for row in sessions])
        self.assertEqual({"harness": "opencode", "source": "opencode_server", "url": url, "status": "partial", "version": "1.18.30",
                          "error": None, "detail": "opencode_invalid_session_records", "rejected": 5}, sources[0])
        self.assertEqual({"harness": "opencode", "session_id": "ses_last", "workspace": ws, "runtime_status": "retry",
                          "runtime_reason": "server_status", "source": "opencode_server", "url": url,
                          "updated_at": self.UPDATED // 1000}, sessions[1])
        # Without a workspace, the server's own project answers: no directory parameter.
        self.assertEqual({("GET", "")}, {(r["method"], r["query"]) for r in server.state["requests"]})
        self.assertNotIn("invented-slug", json.dumps(sessions))

    def test_discovery_is_read_only_and_separates_liveness(self):
        ws, other = str(self.work), str(self.work / "other")
        server, url = self.opencode_server({})
        server.state["listed"] = [
            self.listed("ses_invented", ws), self.listed("ses_child", ws, parentID="ses_invented"),
            self.listed("ses_gone", ws, time={"created": 1, "updated": 2, "archived": 3}), self.listed("ses_busy", other),
            self.listed("ses_odd_time", other, time={"created": 1, "updated": -1500})]
        server.state["status"] = {"ses_busy": {"type": "busy"}}
        data = self.saved("data", [("ses_invented", None, ws, self.UPDATED, None),
                                   ("ses_saved", None, "/invented/saved", 1788980000000, None),
                                   ("ses_archived", None, "/invented/x", 5, 6),
                                   ("ses_subagent", "ses_saved", "/invented/saved", 7, None)])
        with closing(socket.socket()) as unused:
            unused.bind(("127.0.0.1", 0))
            closed = "http://127.0.0.1:%d" % unused.getsockname()[1]
        urls = ["http://user:invented-secret@127.0.0.1:4096", "http://127.attacker.example:4096", url, closed]
        sessions, sources = self.discover(data, *urls)
        text = json.dumps([sessions, sources])
        self.assertFalse([word for word in ("invented-secret", "attacker") if word in text])
        self.assertEqual([("opencode_server", "unavailable", "invalid_opencode_url", None),
                          ("opencode_server", "unavailable", "opencode_url_must_be_loopback", None),
                          ("opencode_server", "ok", None, url),
                          ("opencode_server", "unavailable", "opencode_unreachable", closed),
                          ("opencode_saved", "ok", None, "-")],
                         [(s["source"], s["status"], s["error"], s.get("url", "-")) for s in sources])
        self.assertEqual(self.saved_source(data), sources[4])
        found = {row["session_id"]: row for row in sessions}
        self.assertEqual(["ses_busy", "ses_invented", "ses_odd_time", "ses_saved"], sorted(found))
        self.assertEqual(4, len(sessions))
        self.assertEqual(("busy", "idle"), (found["ses_busy"]["runtime_status"], found["ses_invented"]["runtime_status"]))
        self.assertEqual("opencode_server", found["ses_invented"]["source"])  # A server record wins over its saved duplicate.
        self.assertEqual(-2, found["ses_odd_time"]["updated_at"])  # Floor division.
        self.assertEqual({"harness": "opencode", "session_id": "ses_saved", "workspace": "/invented/saved", "runtime_status": "unknown",
                          "runtime_reason": "saved_metadata_only", "source": "opencode_saved", "url": None,
                          "updated_at": 1788980000}, found["ses_saved"])
        # With a workspace, both listing requests name it, and only its sessions are reported.
        del server.state["requests"][:]
        sessions, _ = self.discover(data, url, workspace=ws)
        self.assertEqual(["ses_invented"], [row["session_id"] for row in sessions])
        scoped = [r for r in server.state["requests"] if r["path"] in ("/session", "/session/status")]
        self.assertEqual([[ws], [ws]], [urllib.parse.parse_qs(r["query"]).get("directory") for r in scoped])
        self.assertEqual({"GET"}, {r["method"] for r in server.state["requests"]})

    def test_malformed_saved_rows_preserve_valid_addresses_and_limit_diagnostics(self):
        ws = str(self.work)
        data = self.saved("data", [("ses_first", None, ws, 5000, None), ("bad_id", None, ws, 4000, None),
                                   ("ses_empty", None, "", 3000, None), (17, None, ws, 2500, None),
                                   ("ses_last", None, ws, 2000.5, None), ("ses_older", None, ws, None, None)])
        sessions, sources = self.discover(data)
        self.assertEqual([("ses_first", 5), ("ses_last", None), ("ses_older", None)],
                         [(row["session_id"], row["updated_at"]) for row in sessions])
        self.assertEqual(self.saved_source(data, "partial", detail="opencode_invalid_saved_metadata", rejected=3), sources[-1])
        # 51 unarchived roots: the newest 50 are read, and two of them are malformed.
        rows = [("bad" if i == 3 else "ses_%02d" % i, None, "" if i == 7 else ws, 10000 - i, None) for i in range(51)]
        data = self.saved("limited", rows)
        sessions, sources = self.discover(data)
        self.assertEqual(48, len(sessions))
        self.assertNotIn("ses_50", [row["session_id"] for row in sessions])
        self.assertEqual(self.saved_source(data, "partial", detail="opencode_saved_session_limit_reached", rejected=2), sources[-1])
        data = self.saved("exact", [row for row in rows[:50] if row[0] != "bad" and row[2]])
        self.assertEqual(self.saved_source(data), self.discover(data)[1][-1])

    def test_saved_text_that_is_not_utf8_rejects_only_its_rows(self):
        rows = [("ses_first", None, "/invented/first", 5000, None), ("ses_bad_id", None, "/invented/bad-id", 4000, None),
                ("ses_bad_directory", None, "/invented/bad-directory", 3000, None), ("ses_last", None, "/invented/last", 2000, None)]
        data = self.saved("data", rows, "id=CAST(x'ff' AS TEXT) WHERE id='ses_bad_id'",
                          "directory=CAST(x'fe' AS TEXT) WHERE id='ses_bad_directory'")
        sessions, sources = self.discover(data)
        self.assertEqual(["ses_first", "ses_last"], [row["session_id"] for row in sessions])
        self.assertEqual(self.saved_source(data, "partial", detail="opencode_invalid_saved_metadata", rejected=2), sources[-1])

    def test_saved_noninteger_timestamps_are_absent_not_rejected(self):
        names = ("ses_invalid_text", "ses_blob", "ses_text", "ses_float", "ses_null", "ses_integer")
        rows = [(name, None, "/invented", updated, None) for name, updated in zip(names, (1, 2, "1234", 2000.5, None, -1500))]
        data = self.saved("data", rows, "time_updated=CAST(x'ff' AS TEXT) WHERE id='ses_invalid_text'",
                          "time_updated=x'fe' WHERE id='ses_blob'")
        sessions, sources = self.discover(data)
        self.assertEqual({name: -2 if name == "ses_integer" else None for name in names},
                         {row["session_id"]: row["updated_at"] for row in sessions})
        self.assertEqual(6, len(sessions))
        self.assertEqual(self.saved_source(data), sources[-1])

    def test_saved_limit_sentinel_is_existence_only_even_with_invalid_text(self):
        # TEXT timestamps sort ahead of the malformed sentinel's leading 'a'.
        rows = [("ses_%02d" % i, None, "/invented", "z%03d" % (51 - i), None) for i in range(51)]
        for column in ("id", "directory", "time_updated"):
            with self.subTest(column=column):
                data = self.saved("sentinel-" + column, rows, "%s=CAST(x'61ff' AS TEXT) WHERE rowid=51" % column)
                last = self.sql("SELECT rowid FROM session ORDER BY time_updated DESC LIMIT 1 OFFSET 50", path=self.database(data))
                self.assertEqual([(51,)], last, "the sentinel must be the 51st selected row")
                sessions, sources = self.discover(data)
                self.assertEqual(["ses_%02d" % i for i in range(50)], [row["session_id"] for row in sessions])
                self.assertEqual(self.saved_source(data, "partial", detail="opencode_saved_session_limit_reached"), sources[-1])

    def test_saved_rejection_at_row_fifty_does_not_backfill_from_sentinel(self):
        rows = [("ses_%02d" % i, None, "/invented", 10000 - i, None) for i in range(51)]
        data = self.saved("data", rows, "directory=CAST(x'ff' AS TEXT) WHERE rowid=50")
        sessions, sources = self.discover(data)
        self.assertEqual(["ses_%02d" % i for i in range(49)], [row["session_id"] for row in sessions])
        self.assertEqual(self.saved_source(data, "partial", detail="opencode_saved_session_limit_reached", rejected=1), sources[-1])

    def test_saved_storage_failures_remain_unavailable(self):
        corrupt, schema = self.tmp / "not-sqlite", self.tmp / "missing-schema"
        for data in (corrupt, schema):
            self.database(data).parent.mkdir(parents=True)
        self.database(corrupt).write_bytes(b"invented invalid SQLite file")
        with closing(sqlite3.connect(self.database(schema))) as db, db:
            db.execute("CREATE TABLE unrelated (value)")
        for data, error in ((corrupt, "opencode_saved_database_corrupt"), (schema, "opencode_saved_database_unavailable"),
                            (self.tmp / "absent", "opencode_saved_metadata_missing")):
            with self.subTest(error=error):
                sessions, sources = self.discover(data)
                self.assertEqual(([], self.saved_source(data, "unavailable", error=error)), (sessions, sources[-1]))

    def test_saved_metadata_default_follows_xdg_data_home(self):
        data = self.saved("xdg", [("ses_xdg", None, "/invented/xdg", 42000, None)])
        sessions, sources = self.discover(data)
        self.assertEqual((["ses_xdg"], str(self.tmp / "xdg/opencode/opencode.db")),
                         ([row["session_id"] for row in sessions], sources[-1]["path"]))
        # An empty XDG_DATA_HOME is no data home: the default under the home directory is read.
        sessions, sources = self.discover("")
        self.assertEqual(([], str(self.home / ".local/share/opencode/opencode.db"), "opencode_saved_metadata_missing"),
                         (sessions, sources[-1]["path"], sources[-1]["error"]))


class ContendedWrites(HtalkCase):
    """What a sender relies on while another process holds the mailbox. Each test waits on the message
    file its send reads, on what the database refuses, on the fake client or on the send's exit."""

    def setUp(self):
        super().setUp()
        self.add_peer("alice")
        self.add_peer("bob")
        # A FIFO opens for writing only while a reader holds it. That shows from outside when a send has
        # opened its message file and when it has read it to the end.
        self.pipe = self.tmp / "pipe"
        if hasattr(os, "mkfifo"):
            os.mkfifo(self.pipe)

    def hold(self, statement):
        db = sqlite3.connect(self.db, isolation_level=None)
        self.addCleanup(db.close)
        db.execute(statement)
        return db

    def running(self, process, otherwise):
        if process.poll() is not None:
            self.fail("%s: %r" % (otherwise, process.communicate()))

    def reader(self, process):
        """The write end of the pipe while the send holds its read end, else None."""
        self.running(process, "send exited while it should be waiting")
        try:
            return os.fdopen(os.open(self.pipe, os.O_WRONLY | os.O_NONBLOCK), "wb")
        except OSError as exc:
            if exc.errno != errno.ENXIO:
                raise

    def give_body(self, process, data, before=lambda: None):
        """Hand the send its body and return once it has read the body to the end."""
        with wait_for(lambda: self.reader(process), message="send to open its message file") as out:
            before()
            out.write(data)

        def done():
            still = self.reader(process)
            if still is None:
                return True
            still.close()
            return False

        wait_for(done, message="send to read its message file to the end")

    def keeps_waiting(self, process):
        """A send that cannot write yet neither fails nor returns."""
        try:
            process.wait(timeout=0.5)
        except subprocess.TimeoutExpired:
            pass
        self.running(process, "send did not wait")

    def committing(self, process):
        self.running(process, "send did not wait for the reader")
        return turned_away(self.db)

    def need_pipe(self):
        if not self.pipe.exists():
            self.skipTest("this system has no named pipe in the file tree to watch a send through")

    def test_contended_send_saves_the_body_it_read_once(self):
        self.need_pipe()
        body = self.tmp / "body.txt"
        body.symlink_to(self.pipe)
        changed = self.tmp / "changed.txt"
        changed.write_text("Changed while waiting")
        writer = self.hold("BEGIN IMMEDIATE")
        proc = self.spawn("--as", "alice", "send", "bob", "--message-file", body, "--no-notify")
        # The send holds the file it opened. Before it has read a byte, its path names other contents.
        self.give_body(proc, b"First\r\nSecond\rThird", before=lambda: changed.replace(body))
        self.keeps_waiting(proc)
        writer.rollback()
        sent = self.finish(proc)
        self.assertEqual("First\nSecond\nThird", sent["body"])
        self.assertEqual([(sent["id"], sent["body"])], self.sql("SELECT id,body FROM messages"))

    def test_send_blocked_at_commit_by_a_reader_finishes_once(self):
        reader = self.hold("BEGIN")
        reader.execute("SELECT * FROM peers").fetchall()
        proc = self.spawn("--as", "alice", "send", "bob", "--message", "Held commit", "--no-notify")
        # The reader lets the send write and keeps it from committing.
        wait_for(lambda: self.committing(proc), message="send to wait for its commit")
        reader.rollback()
        sent = self.finish(proc)
        self.assertEqual([(sent["id"],)], self.sql("SELECT id FROM messages"))

    def test_interrupt_while_another_writer_holds_the_mailbox_saves_nothing(self):
        self.need_pipe()
        writer = self.hold("BEGIN IMMEDIATE")
        # A registration waits for the same writer. Nobody interrupts it.
        waiting = self.spawn(*self.peer_words("carol"))
        self.keeps_waiting(waiting)
        proc = self.spawn("--as", "alice", "send", "bob", "--message-file", self.pipe, "--no-notify", group=True)
        self.give_body(proc, b"Interrupted")
        self.keeps_waiting(proc)
        proc.send_signal(INTERRUPT)
        interrupted = self.finish(proc, code=130)
        self.assertEqual(("interrupted", None), (interrupted["state"], interrupted["message_id"]))
        self.running(waiting, "the interrupted send waited as long as the writer nobody interrupted")
        with self.assertRaises(ProcessLookupError, msg="the interrupted send left a process behind"):
            os.killpg(proc.pid, 0)
        writer.rollback()
        self.assertEqual("carol", self.finish(waiting)["name"])
        self.assertEqual([], self.sql("SELECT id FROM messages"))

    def test_send_gives_up_five_seconds_after_it_finds_the_mailbox_held(self):
        writer = self.hold("BEGIN IMMEDIATE")
        started = time.monotonic()
        self.error("--as", "alice", "send", "bob", "--message", "Too late", "--no-notify", error="database is locked")
        waited = time.monotonic() - started
        writer.rollback()
        # The budget is five seconds by the clock, so this test asserts a duration. Its upper bound leaves
        # room for a machine that starts a process slowly, and none for a wait that counts its pauses.
        self.assertTrue(4.5 < waited < 10, "the send gave up after %.2f seconds" % waited)
        self.assertEqual([], self.sql("SELECT id FROM messages"))

    def test_senders_that_write_at_once_lose_no_message(self):
        ids = [str(uuid.uuid4()) for _ in range(16)]
        senders = [self.spawn("--as", "alice", "send", "bob", "--id", one, "--message", "From " + one, "--no-notify")
                   for one in ids]
        for sender in senders:
            self.assertTrue(self.finish(sender)["created"])
        saved = self.sql("SELECT id, body, seq FROM messages")
        self.assertEqual(sorted((one, "From " + one) for one in ids), sorted(row[:2] for row in saved))
        self.assertEqual(16, len({row[2] for row in saved}))

    def test_repeated_send_returns_during_the_first_notification(self):
        self.codex_recipient("dave")
        words = ["--as", "alice", "send", "dave", "--id", str(uuid.uuid4()), "--message", "One notice"]
        self.configure(codex_queue={"mode": "block"})
        proc = self.spawn(*words)
        self.started("codex_queue")
        repeated = self.htalk(*words)
        self.assertIsNone(proc.poll(), "the repeated send waited for the first notification")
        self.assertFalse(repeated["created"])
        self.release("codex_queue")
        first = self.finish(proc, code=2)
        self.assertEqual((True, repeated["id"], "submission_unknown"), (first["created"], first["id"], first["submission"]))
        self.assertEqual(1, len(self.calls("codex", ["queue"])))


if __name__ == "__main__":
    unittest.main()
