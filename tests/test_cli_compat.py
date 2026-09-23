"""Black-box compatibility tests for the documented htalk CLI, JSON and SQLite contracts.

The suite runs the executable selected by HTALK_TEST_COMMAND (see compat_support).
Fake claude/codex executables, a fake Claude messaging socket and a loopback
OpenCode server stand in for clients. No test imports the implementation.
"""
from contextlib import closing
import fcntl
import json
import os
from pathlib import Path
import shlex
import signal
import socket
import sqlite3
import subprocess
import sys
import time
import unittest
import uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from compat_support import MESSAGE_KEYS, PEER_KEYS, ROW_KEYS, HtalkCase, process_start, wait_for  # noqa: E402

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


def words_of(command):
    return shlex.split(command)


class DatabaseSelection(HtalkCase):
    def test_only_peer_add_creates_the_database(self):
        message_id = str(uuid.uuid4())
        commands = (["migrate"], ["peer", "list"], ["peer", "list", "--all"], ["peer", "check", "bob"], ["peer", "retire", "bob"],
                    ["peer", "restore", "bob"], ["inbox"], ["--as", "alice", "inbox"], ["--as", "alice", "sent"],
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
        self.assertEqual(PEER_KEYS, set(bob))
        self.assertEqual(["bob"], [peer["name"] for peer in self.htalk("peer", "list")["peers"]])
        # Created privately, independent of the caller's umask.
        self.assertEqual(0, self.db.stat().st_mode & 0o077)
        self.assertEqual(0, self.db.parent.stat().st_mode & 0o077)

    def test_path_precedence_and_resolution(self):
        address = ["peer", "add", "bob", "--harness", "claude", "--session", str(uuid.uuid4()), "--workspace", str(self.work)]
        option, env_db = self.tmp / "option.sqlite3", self.tmp / "env.sqlite3"
        xdg = self.tmp / "xdg"
        self.htalk(*address, db=option, env={"HTALK_DB": str(env_db), "XDG_DATA_HOME": str(xdg)})
        self.assertEqual((True, False), (option.exists(), env_db.exists()))
        self.htalk(*address, db=False, env={"HTALK_DB": str(env_db), "XDG_DATA_HOME": str(xdg)})
        self.assertTrue(env_db.exists())
        self.assertFalse(xdg.exists())
        self.htalk(*address, db=False, env={"HTALK_DB": "", "XDG_DATA_HOME": str(xdg)})
        self.assertTrue((xdg / "harness-talk/mail.sqlite3").exists())
        self.htalk(*address, db=False)
        self.assertTrue((self.home / ".local/share/harness-talk/mail.sqlite3").exists())
        # A relative or ~ path resolves against the working directory or HOME, in errors too.
        missing = self.error("peer", "list", db="rel/mail.sqlite3", cwd=self.work, error="database_not_found")
        self.assertEqual(str(self.work / "rel/mail.sqlite3"), missing["resolved_path"])
        self.htalk(*address, db="~/tilde.sqlite3")
        self.assertTrue((self.home / "tilde.sqlite3").exists())
        self.assertFalse((self.tmp / "~").exists())
        self.assertEqual(["bob"], [p["name"] for p in self.htalk("peer", "list", db=self.home / "tilde.sqlite3")["peers"]])

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
        self.error("peer", "add", "bob", "--harness", "claude", "--session", str(uuid.uuid4()),
                   "--workspace", str(self.work), error="unsupported_database_version")
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
        self.assertEqual(ROW_KEYS, set(self.columns("messages", path)))
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
        self.assertEqual(0o600, backup.stat().st_mode & 0o777)
        self.assertEqual(0o700, backup.parent.stat().st_mode & 0o777)
        self.assert_current_schema()
        self.assertEqual(["alice", "bob", "builder"], [peer["name"] for peer in listed])
        self.assertEqual([None] * 3, [peer["url"] for peer in listed])
        self.assertEqual([None] * 3, [peer["retired_at"] for peer in listed])
        self.assertEqual("/nowhere/codex.sock", listed[2]["socket"])
        inbox = self.htalk("--as", "bob", "inbox")
        self.assertEqual([ids["q1"], ids["q2"], ids["r3"]], [m["id"] for m in inbox["messages"]])
        shown = self.htalk("--as", "alice", "show", ids["q1"])
        self.assertEqual((100.0, "submitted", "claude_socket_bytes_written", None),
                         (shown["ack_at"], shown["submission"], shown["notification_detail"], shown["wait_returned_at"]))
        answered = self.htalk("--as", "bob", "show", ids["q3"])
        self.assertEqual(("reply_received", ids["r3"], "Old answer"), (answered["state"], answered["reply"]["id"], answered["reply"]["body"]))
        # An identical registration of a migrated peer is accepted unchanged.
        self.assertEqual(listed[0], self.add_peer("alice", "claude", ids["alice"]))

    def test_early_version_two_migrates_on_ordinary_read(self):
        ids = self.legacy_v1(self.db)
        self.sql("ALTER TABLE peers ADD COLUMN url TEXT")
        self.sql("PRAGMA user_version=2")
        before = self.snapshot(self.db)
        self.htalk("--as", "bob", "show", ids["q1"])
        backup, = self.backups()
        self.assertEqual(before, self.snapshot(backup))
        self.assert_current_schema()
        self.assertIsNone(self.htalk("--as", "bob", "show", ids["q1"])["wait_returned_at"])
        self.htalk("peer", "retire", "bob")
        self.assertEqual(2, len(self.htalk("peer", "list")["peers"]))

    def test_migration_requires_explicit_path_and_resolves_tilde(self):
        path = self.home / "old.sqlite3"
        self.legacy_v1(path)
        before = path.read_bytes()
        self.error("migrate", db=False, env={"HTALK_DB": str(path)}, error="migrate_requires_explicit_db")
        self.error("migrate", db=False, error="migrate_requires_explicit_db")
        self.assertEqual(before, path.read_bytes())
        self.htalk("peer", "list", db="~/old.sqlite3")
        self.htalk("migrate", db="~/old.sqlite3")
        self.assertEqual(1, len(self.backups(path)))
        self.assert_current_schema(path)
        self.assertEqual(3, len(self.htalk("peer", "list", db="~/old.sqlite3")["peers"]))

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
            process.send_signal(signal.SIGINT)
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
            process.send_signal(signal.SIGINT)
            self.finish(process, code=130)
            reader.rollback()
        self.assertEqual(before, self.snapshot(self.db))
        backup, = self.backups()
        self.assertEqual(before, self.snapshot(backup))

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
            tuple(request[k] for k in ("submission", "notification_detail", "notification_started_at", "notification_finished_at")))
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
        self.assertIsNone(back["notification_started_at"])
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


    def test_unknown_native_adapter_keeps_mail_readable(self):
        self.pull("alice")
        self.add_peer("bob")
        self.sql("UPDATE peers SET harness='unknown-future-adapter' WHERE name='bob'")
        self.assertEqual(2, len(self.htalk("peer", "list")["peers"]))
        self.error("peer", "check", "bob", error="adapter_unavailable")
        message_id = str(uuid.uuid4())
        request = self.htalk("--as", "alice", "send", "bob", "--message", "Question", "--id", message_id, code=2)
        self.assertEqual("adapter_unavailable", request["notification_detail"])
        self.assertEqual("not_submitted", request["submission"])
        self.assertEqual(1, self.htalk("--as", "bob", "inbox")["total"])
        self.assertEqual("Question", self.htalk("--as", "bob", "show", message_id)["body"])
        retry = self.htalk("--as", "alice", "send", "bob", "--message", "Question", "--id", message_id)
        self.assertFalse(retry["created"])
        self.assertEqual(request["notification_started_at"], retry["notification_started_at"])
        self.htalk("--as", "bob", "ack", message_id)
        self.assertEqual([], self.calls())


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
                 ("bob", "claude", uuid_session, str(self.tmp / "file"), [], "workspace_must_be_a_directory"))
        for name, harness, session, workspace, options, code in coded:
            with self.subTest(name=name, harness=harness, options=options):
                error = self.error("peer", "add", name, "--harness", harness, "--session", session,
                                   "--workspace", workspace, *options, error=code)
                self.assertIn("peer list", error["recovery"]["peers"])
                self.assertNotIn("secret", json.dumps(error))
        # Exception text for these is not a documented code; only the failure is.
        for session, workspace in (("not-a-uuid", work), (uuid_session, str(self.tmp / "missing"))):
            with self.subTest(session=session, workspace=workspace):
                self.error("peer", "add", "bob", "--harness", "claude", "--session", session, "--workspace", workspace)
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
        self.assertEqual(("not_submitted", None, None, None, None),
                         (question["submission"], question["notification_started_at"], question["notification_detail"],
                          question["ack_at"], question["wait_returned_at"]))
        self.assertEqual(str(uuid.UUID(question["id"])), question["id"])
        qid = question["id"]
        inbox = self.htalk("--as", "bob", "inbox")
        self.assertEqual(([qid], 1, 0), ([m["id"] for m in inbox["messages"]], inbox["total"], inbox["omitted"]))
        self.assertEqual("Which case?", inbox["messages"][0]["body"])
        self.assertIn("ack", inbox["next_action"])
        self.error("--as", "eve", "show", qid, error="message_not_addressed_to_peer")
        self.error("--as", "alice", "show", str(uuid.uuid4()), error="unknown_message")
        for actor in ("alice", "eve"):
            self.error("--as", actor, "ack", qid, error="only_recipient_can_ack")
        self.assertIsNone(self.htalk("--as", "alice", "show", qid)["ack_at"])
        # Acknowledging records reading only; the question stays open.
        read = self.htalk("--as", "bob", "ack", qid)
        self.assertIsNotNone(read["ack_at"])
        self.assertEqual(("saved", None), (read["state"], read["reply"]))
        self.assertIn("open until you reply", read["next_action"])
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
        self.assertIsNotNone(waited["reply"]["wait_returned_at"])
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

    def test_wait_timeout_and_its_recovery(self):
        question = self.htalk("--as", "alice", "send", "bob", "--message", "Q", "--no-notify")
        for seconds in ("0", "0.2"):
            with self.subTest(seconds=seconds):
                waited = self.htalk("--as", "alice", "wait", question["id"], "--seconds", seconds)
                self.assertEqual(("timeout", "saved", None), (waited["wait_ended"], waited["state"], waited["reply"]))
                self.assertEqual(["htalk", "--db", str(self.db), "--as", "alice", "wait", question["id"], "--seconds", "45"],
                                 words_of(waited["recovery"]["wait"]))
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
        while "recovery" in pages[-1]:
            self.assertIn("recovery.next_page", pages[-1]["next_action"])
            pages.append(self.recover(pages[-1]["recovery"]["next_page"]))
        return pages

    def test_sent_pages_newest_first_without_gaps(self):
        ids = self.insert_requests("alice", "bob", 25)
        pages = self.follow("--as", "alice", "sent", "--limit", "10")
        self.assertEqual([(10, 25, 15), (10, 25, 5), (5, 25, 0)],
                         [(len(p["messages"]), p["total"], p["omitted"]) for p in pages])
        self.assertEqual(ids[::-1], [m["id"] for p in pages for m in p["messages"]])
        last = pages[0]["messages"][-1]["seq"]
        self.assertEqual(["htalk", "--db", str(self.db), "--as", "alice", "sent", "--limit", "10", "--before-seq", str(last)],
                         words_of(pages[0]["recovery"]["next_page"]))
        seqs = [m["seq"] for p in pages for m in p["messages"]]
        self.assertEqual(sorted(seqs, reverse=True), seqs)
        default = self.htalk("--as", "alice", "sent")
        self.assertEqual((20, 5), (len(default["messages"]), default["omitted"]))
        older = self.htalk("--as", "alice", "sent", "--before-seq", str(seqs[-5]))
        self.assertEqual((ids[3::-1], 25, 0), ([m["id"] for m in older["messages"]], older["total"], older["omitted"]))
        bodies = self.follow("--as", "alice", "sent", "--limit", "20", "--bodies")
        self.assertIn("--bodies", words_of(bodies[0]["recovery"]["next_page"]))
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
                          str(pages[0]["messages"][-1]["seq"])], words_of(pages[0]["recovery"]["next_page"]))
        self.assertNotIn("recovery", pages[-1])
        after = self.htalk("--as", "bob", "inbox", "--after-seq", str(pages[1]["messages"][-1]["seq"]))
        self.assertEqual(open_ids[4:], [m["id"] for m in after["messages"]])
        self.assertEqual((5, 0), (after["total"], after["omitted"]))
        # Unacknowledged answers are incoming work for the requester.
        self.assertEqual([answer], [m["id"] for m in self.htalk("--as", "alice", "inbox")["messages"]])


class Recovery(HtalkCase):
    def test_recovery_commands_are_executable_with_a_quoted_database_path(self):
        self.db = self.tmp / "mail 'quoted' $draft" / "m.sqlite3"
        self.add_peer("builder")
        self.add_peer("reviewer")
        question = self.htalk("--as", "builder", "send", "reviewer", "--message", "Question", "--no-notify")
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
        bob = self.add_peer("bob")
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
        self.assertIn("recipient peer is retired", answer["next_action"])
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
        question = self.htalk("send", "builder", "--message", "Question", "--no-notify", env=native)
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


class Notifications(HtalkCase):
    def test_claude_notice_is_one_frame_with_a_show_command_and_no_body(self):
        self.add_peer("alice")
        bob, listener = self.claude_recipient("bob")
        sent = self.htalk("--as", "alice", "send", "bob", "--message", "Private body 42")
        self.assertEqual(("submitted", "claude_socket_bytes_written", True),
                         (sent["submission"], sent["notification_detail"], sent["created"]))
        self.assertIsNotNone(sent["notification_started_at"])
        self.assertIsNotNone(sent["notification_finished_at"])
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
        self.add_peer("bob")
        chosen = str(uuid.uuid4())
        failed = self.htalk("--as", "alice", "send", "bob", "--id", chosen, "--message", "Q", code=2)
        self.assertEqual(("not_submitted", "recipient_unavailable", True),
                         (failed["submission"], failed["notification_detail"], failed["created"]))
        self.assertIsNotNone(failed["notification_started_at"])
        self.assertIn("Never repeat an uncertain notification", failed["next_action"])
        self.assertEqual(1, len(self.calls("claude", ["agents"])))
        retry = self.htalk("--as", "alice", "send", "bob", "--id", chosen, "--message", "Q")
        self.assertEqual((False, "not_submitted", failed["notification_started_at"]),
                         (retry["created"], retry["submission"], retry["notification_started_at"]))
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
                self.assertIsNotNone(result["notification_finished_at"])
                self.assertEqual([], listener.frames())
                self.htalk("peer", "restore", "bob")
        # Acknowledgment does not answer the question.
        self.assertEqual(2, self.htalk("--as", "bob", "inbox")["total"])

    def test_answer_returned_by_the_requester_wait_is_not_notified(self):
        self.add_peer("alice")
        self.add_peer("bob")
        question = self.htalk("--as", "alice", "send", "bob", "--message", "Q", "--no-notify")["id"]
        waiting = self.spawn("--as", "alice", "wait", question, "--seconds", "20")
        wait_for(self.waits, message="the wait registration")
        self.assertEqual([(question, "alice")], self.waits())
        # Without that receipt this notice would fail with recipient_unavailable and exit 2.
        answer = self.htalk("--as", "bob", "reply", question, "--message", "Answer")
        self.assertEqual(("not_submitted", "returned_by_recipient_wait", None),
                         (answer["submission"], answer["notification_detail"], answer["ack_at"]))
        self.assertIsNotNone(answer["wait_returned_at"])
        self.assertIn("no client notice was sent", answer["next_action"])
        received = self.finish(waiting)
        self.assertEqual((answer["id"], "Answer"), (received["reply"]["id"], received["reply"]["body"]))
        self.assertIsNotNone(received["reply"]["wait_returned_at"])
        self.assertIn("ack_after_reading", received["recovery"])
        self.assertEqual([], self.waits())
        self.assertEqual([answer["id"]], [m["id"] for m in self.htalk("--as", "alice", "inbox")["messages"]])

    def test_send_wait_that_returns_an_answer_exits_zero_despite_an_unconfirmed_notice(self):
        self.add_peer("alice")
        self.add_peer("bob")
        chosen = str(uuid.uuid4())
        sending = self.spawn("--as", "alice", "send", "bob", "--id", chosen, "--message", "Q", "--wait", "20")
        wait_for(self.waits, message="the send --wait registration")
        self.htalk("--as", "bob", "reply", chosen, "--message", "Answer", "--no-notify")
        result = self.finish(sending, code=0)
        self.assertEqual((chosen, True, "not_submitted", "recipient_unavailable", "Answer"),
                         (result["id"], result["created"], result["submission"], result["notification_detail"],
                          result["reply"]["body"]))
        self.assertIsNotNone(result["reply"]["wait_returned_at"])
        self.assertEqual("option", result["actor_source"])


class Interrupts(HtalkCase):
    def interrupt(self, process, code=130):
        process.send_signal(signal.SIGINT)
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
        self.assertEqual(("submission_unknown", None), (shown["submission"], shown["notification_finished_at"]))
        self.assertIsNotNone(shown["notification_started_at"])
        self.assertEqual([chosen], [m["id"] for m in self.recover(result["recovery"]["sent"])["messages"]])
        self.configure(codex_queue={"mode": "ok", "queue_id": str(uuid.uuid4())})
        retry = self.htalk("--as", "alice", "send", "bob", "--id", chosen, "--message", "Q")
        self.assertEqual((False, "submission_unknown"), (retry["created"], retry["submission"]))
        self.assertEqual(1, len(self.calls("codex", ["queue"])))
        self.assertEqual([chosen], [m["id"] for m in self.htalk("--as", "bob", "inbox")["messages"]])

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
                         (before["submission"], before["notification_finished_at"]))
        self.assertIsNotNone(before["notification_started_at"])
        self.kill(sending)

        for retry in self.two_retries(chosen):
            self.assertEqual(("Question?", before["created_at"], "submission_unknown", None),
                             (retry["body"], retry["created_at"], retry["submission"],
                              retry["notification_finished_at"]))
        self.assertEqual([(1,)], self.sql("SELECT COUNT(*) FROM messages WHERE id=?", (chosen,)))
        self.assertEqual([chosen], [m["id"] for m in self.htalk("--as", "bob", "inbox")["messages"]])
        acknowledged = self.htalk("--as", "bob", "ack", chosen)
        self.assertIsNotNone(acknowledged["ack_at"])
        self.assertEqual(("submission_unknown", None, "pending"),
                         (acknowledged["submission"], acknowledged["notification_finished_at"],
                          acknowledged["notification_cleanup"]["status"]))
        repeated_ack = self.recover(acknowledged["recovery"]["retry_notification_cleanup"])
        self.assertEqual((acknowledged["ack_at"], "pending"),
                         (repeated_ack["ack_at"], repeated_ack["notification_cleanup"]["status"]))
        # This gated fake exits unsuccessfully without a receipt when released.
        # Observe the late failure, without claiming to simulate late acceptance.
        child_pid = self.calls("codex", ["queue"])[0]["pid"]

        def fake_is_running():
            try:
                return str(self.bin).encode() in Path("/proc/%d/cmdline" % child_pid).read_bytes()
            except FileNotFoundError:
                return False

        self.assertTrue(fake_is_running(), "the blocked fake must outlive the killed sender")
        self.release("codex_queue")
        wait_for(lambda: not fake_is_running(), message="the orphaned fake client to exit")
        after_child = self.htalk("--as", "bob", "show", chosen)
        self.assertEqual(("submission_unknown", None, acknowledged["ack_at"]),
                         (after_child["submission"], after_child["notification_finished_at"],
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
        self.assertIsNotNone(before["notification_finished_at"])
        self.kill(sending)
        self.assertTrue(self.waits(), "SIGKILL must leave the registered poll behind")

        for retry in self.two_retries(chosen):
            self.assertEqual(("submitted", before["created_at"], before["notification_finished_at"]),
                             (retry["submission"], retry["created_at"], retry["notification_finished_at"]))
        answer = self.htalk("--as", "bob", "reply", chosen, "--message", "Answer")
        self.assertEqual("submitted", answer["submission"])
        self.assertIsNotNone(answer["notification_finished_at"])
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
                self.assertIsNone(unfinished["notification_finished_at"])
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
                            self.assertEqual(result["submission"] == "not_submitted",
                                             result["notification_started_at"] is None)
                            self.assertIsNone(result["notification_finished_at"])

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
        self.assertIn("next_action", found)
        self.assertIn("scope", found)
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


class WriteAdmission(HtalkCase):
    def setUp(self):
        super().setUp()
        self.add_peer("alice")
        self.codex_recipient("bob")
        self.turn = Path(str(self.db) + "-htalk-turn")

    def hold(self, statement):
        db = sqlite3.connect(self.db, isolation_level=None)
        self.addCleanup(db.close)
        db.execute(statement)
        return db

    def test_contended_send_keeps_the_first_message_file_contents(self):
        body = self.tmp / "body.txt"
        body.write_bytes(b"First\r\nSecond\rThird")
        writer = self.hold("BEGIN IMMEDIATE")
        proc = self.spawn("--as", "alice", "send", "bob", "--message-file", body, "--no-notify")
        wait_for(self.turn.exists, message="send waiting after reading its body")
        body.write_text("Changed while waiting")
        writer.rollback()
        sent = self.finish(proc)
        self.assertEqual("First\nSecond\nThird", sent["body"])
        self.assertEqual([(sent["id"], sent["body"])], self.sql("SELECT id,body FROM messages"))

    def test_commit_wait_does_not_restart_the_write(self):
        reader = self.hold("BEGIN")
        reader.execute("SELECT * FROM peers").fetchall()
        proc = self.spawn("--as", "alice", "send", "bob", "--message", "Held commit", "--no-notify")
        wait_for(Path(str(self.db) + "-journal").exists, message="write reached its journal")
        time.sleep(.1)
        self.assertIsNone(proc.poll())
        self.assertFalse(self.turn.exists())
        reader.rollback()
        sent = self.finish(proc)
        self.assertEqual([(sent["id"],)], self.sql("SELECT id FROM messages"))

    def test_interrupt_while_waiting_for_admission_saves_nothing(self):
        self.turn.mkdir(mode=0o700)
        holder = os.open(self.turn, os.O_RDONLY | os.O_DIRECTORY)
        self.addCleanup(os.close, holder)
        fcntl.flock(holder, fcntl.LOCK_EX)
        writer = self.hold("BEGIN IMMEDIATE")
        proc = self.spawn("--as", "alice", "send", "bob", "--message", "Interrupted", "--no-notify")
        tasks = Path("/proc") / str(proc.pid) / "task"
        wait_for(lambda: len(list(tasks.iterdir())) >= 2, message="admission helper waiting")
        writer.rollback()
        proc.send_signal(signal.SIGINT)
        self.assertEqual("interrupted", self.finish(proc, code=130)["state"])
        self.assertEqual([], self.sql("SELECT id FROM messages"))

    def test_admission_is_released_before_the_notification_attempt(self):
        ident = str(uuid.uuid4())
        words = ["--as", "alice", "send", "bob", "--id", ident, "--message", "One notice"]
        self.configure(codex_queue={"mode": "block"})
        writer = self.hold("BEGIN IMMEDIATE")
        proc = self.spawn(*words)
        wait_for(self.turn.exists, message="send entered admission")
        writer.rollback()
        self.started("codex_queue")
        holder = os.open(self.turn, os.O_RDONLY | os.O_DIRECTORY)
        try:
            fcntl.flock(holder, fcntl.LOCK_EX | fcntl.LOCK_NB)
        finally:
            os.close(holder)
        (self.state / "codex_queue.release").touch()
        self.assertEqual("submission_unknown", self.finish(proc, code=2)["submission"])
        repeated = self.htalk(*words)
        self.assertFalse(repeated["created"])
        self.assertEqual("submission_unknown", repeated["submission"])
        self.assertEqual(1, len(self.calls("codex", ["queue"])))


if __name__ == "__main__":
    unittest.main()
