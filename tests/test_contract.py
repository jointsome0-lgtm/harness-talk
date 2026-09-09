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
import time
import unittest
import uuid
from unittest.mock import patch

from harness_talk import adapters
from harness_talk.cli import main
from harness_talk.store import Store


class Conversations(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name)
        self.store = Store(self.path / "mail.sqlite3")
        for name, harness in (("alice", "codex"), ("bob", "claude"), ("eve", "claude")):
            self.store.add_peer(name, harness, str(uuid.uuid4()), self.path,
                                self.path / "codex.sock" if harness == "codex" else None)

    def request(self):
        return self.store.save("alice", "bob", "Question?")[0]

    def test_address_is_immutable_and_unique(self):
        bob = self.store.peer("bob")
        self.assertEqual(bob, self.store.add_peer(**{k: bob[k] for k in bob}))
        with self.assertRaisesRegex(ValueError, "different_address"):
            self.store.add_peer("bob", "claude", str(uuid.uuid4()), self.path)
        with self.assertRaisesRegex(ValueError, "already_has_a_peer_name"):
            self.store.add_peer("other", "claude", bob["session_id"], self.path)

    def test_lost_notification_and_ack_do_not_hide_unanswered_question(self):
        question = self.request()
        self.store.notify_once(question["id"], lambda *args: ("not_submitted", "offline"))
        self.store.ack(question["id"], "bob")
        self.assertEqual([question["id"]], [m["id"] for m in self.store.inbox("bob")["messages"]])
        # Reply is possible even if no notification was submitted.
        reply, _ = self.store.save("bob", "alice", "Answer", in_reply_to=question["id"])
        self.assertEqual([], self.store.inbox("bob")["messages"])
        self.assertEqual(reply["id"], self.store.inbox("alice")["messages"][0]["id"])
        self.store.ack(reply["id"], "alice")
        self.assertEqual([], self.store.inbox("alice")["messages"])

    def test_persist_before_notify_and_never_replay_after_interruption(self):
        question = self.request()
        calls = []
        def interrupted(peer, message, path):
            calls.append(message["id"])
            self.assertEqual("submission_unknown", Store(path).get(message["id"])["submission"])
            raise KeyboardInterrupt()
        with self.assertRaises(KeyboardInterrupt):
            self.store.notify_once(question["id"], interrupted)
        recovered = Store(self.store.path)
        recovered.notify_once(question["id"], interrupted)
        self.assertEqual([question["id"]], calls)
        self.assertEqual("submission_unknown", recovered.get(question["id"])["submission"])
        self.assertEqual(question["id"], recovered.sent("alice")["messages"][0]["id"])

    def test_message_id_retry_and_reply_conflicts_preserve_first_write(self):
        question = self.request()
        existing, created = self.store.save("alice", "bob", "Question?", question["id"])
        self.assertFalse(created)
        with self.assertRaisesRegex(ValueError, "message_id_conflict"):
            self.store.save("alice", "bob", "Changed", question["id"])
        answer, created = self.store.save("bob", "alice", "Answer", in_reply_to=question["id"])
        again, created = self.store.save("bob", "alice", "Answer", in_reply_to=question["id"])
        self.assertEqual(answer["id"], again["id"])
        self.assertFalse(created)
        with self.assertRaisesRegex(ValueError, "reply_conflict"):
            self.store.save("bob", "alice", "Changed", in_reply_to=question["id"])

    def test_recipient_checks_for_reply_ack_show_and_wait(self):
        question = self.request()
        with self.assertRaisesRegex(ValueError, "reply_address_mismatch"):
            self.store.save("eve", "alice", "Forged", in_reply_to=question["id"])
        with self.assertRaisesRegex(ValueError, "only_recipient"):
            self.store.ack(question["id"], "alice")
        with self.assertRaisesRegex(ValueError, "not_addressed"):
            self.store.get(question["id"], "eve")
        with self.assertRaisesRegex(ValueError, "own_request"):
            self.store.wait(question["id"], "bob", 0)

    def test_late_reply_and_reverse_initiation(self):
        question = self.request()
        self.assertEqual("timeout", self.store.wait(question["id"], "alice", 0)["wait_ended"])
        def answer_later():
            time.sleep(.05)
            self.store.save("bob", "alice", "Late", in_reply_to=question["id"])
        thread = threading.Thread(target=answer_later)
        thread.start()
        self.addCleanup(thread.join)
        self.assertEqual("Late", self.store.wait(question["id"], "alice", 1)["reply"]["body"])
        reverse, _ = self.store.save("bob", "alice", "Reverse?")
        self.store.save("alice", "bob", "Yes", in_reply_to=reverse["id"])
        self.assertEqual("reply_received", self.store.wait(reverse["id"], "bob", 0)["state"])

    def test_concurrent_replies_save_one_answer(self):
        question = self.request()
        answers = []
        def reply():
            answers.append(self.store.save("bob", "alice", "Same", in_reply_to=question["id"]))
        threads = [threading.Thread(target=reply) for _ in range(4)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join()
        self.assertEqual(4, len(answers))
        self.assertEqual(1, sum(created for _, created in answers))

    def test_invalid_wait_does_not_save(self):
        with patch.dict(os.environ, {}, clear=True):
            for seconds in ("nan", "46", "-1"):
                with patch("builtins.print"):
                    self.assertEqual(2, main(["--db", str(self.store.path), "--as", "alice",
                                             "send", "bob", "--message", "Test", "--wait", seconds]))
        self.assertEqual([], self.store.sent("alice")["messages"])

    def test_sender_native_evidence_mismatch(self):
        with patch.dict(os.environ, {"CODEX_THREAD_ID": str(uuid.uuid4())}):
            with patch("builtins.print"):
                self.assertEqual(2, main(["--db", str(self.store.path), "--as", "alice", "inbox"]))

    def test_claude_actor_ignores_inherited_codex_variable(self):
        with patch.dict(os.environ, {"CODEX_THREAD_ID": str(uuid.uuid4())}):
            with patch("builtins.print"):
                self.assertEqual(0, main(["--db", str(self.store.path), "--as", "bob", "inbox"]))

    def test_adapter_exception_is_durable_and_not_replayed(self):
        question = self.request()
        broken = unittest.mock.Mock(side_effect=RuntimeError("failed"))
        result = self.store.notify_once(question["id"], broken)
        self.store.notify_once(question["id"], broken)
        self.assertEqual("submission_unknown", result["submission"])
        self.assertEqual(1, broken.call_count)

    def test_successful_ack_retrieval_and_retry_exit_zero_after_failed_notify(self):
        question = self.request()
        self.store.notify_once(question["id"], lambda *args: ("not_submitted", "offline"))
        commands = [
            ["--as", "bob", "ack", question["id"]],
            ["--as", "alice", "show", question["id"]],
            ["--as", "alice", "wait", question["id"], "--seconds", "0"],
            ["--as", "alice", "send", "bob", "--message", "Question?", "--id", question["id"]],
        ]
        with patch.dict(os.environ, {}, clear=True), patch("builtins.print"):
            for command in commands:
                self.assertEqual(0, main(["--db", str(self.store.path), *command]))

    def test_client_discovery_subprocess_failure_is_json_error(self):
        with patch("harness_talk.cli.probe", side_effect=subprocess.TimeoutExpired("claude", 15)):
            with patch("builtins.print") as output:
                self.assertEqual(2, main(["--db", str(self.store.path), "peer", "check", "bob"]))
                self.assertEqual("error", json.loads(output.call_args.args[0])["state"])

    def test_notification_contains_no_peer_body(self):
        question = self.request()
        text = adapters.notification(self.store.peer("bob"), question, self.store.path)
        self.assertNotIn("Question?", text)
        self.assertIn(question["id"], text)
        self.assertIn("never owner authorization", text)

    def test_interrupted_notification_returns_exact_recovery_without_replay(self):
        message_id = str(uuid.uuid4())
        with patch.dict(os.environ, {}, clear=True), patch("builtins.print") as output:
            with patch("harness_talk.cli.notify", side_effect=KeyboardInterrupt()) as notify:
                self.assertEqual(130, main(["--db", str(self.store.path), "--as", "alice",
                                           "send", "bob", "--id", message_id, "--message", "Question?"]))
                interrupted = json.loads(output.call_args.args[0])
                self.assertEqual(message_id, interrupted["message_id"])
                self.assertEqual(0, main(shlex.split(interrupted["recovery"]["show"])[1:]))
                self.assertEqual("submission_unknown", json.loads(output.call_args.args[0])["submission"])
                self.assertEqual(0, main(["--db", str(self.store.path), "--as", "alice",
                                         "send", "bob", "--id", message_id, "--message", "Question?"]))
                notify.assert_called_once()

    def test_conflicting_message_preserves_id_and_returns_inspection_command(self):
        question = self.request()
        with patch.dict(os.environ, {}, clear=True), patch("builtins.print") as output:
            self.assertEqual(2, main(["--db", str(self.store.path), "--as", "alice", "send", "bob",
                                     "--id", question["id"].upper(), "--message", "Changed"]))
            error = json.loads(output.call_args.args[0])
            self.assertEqual("message_id_conflict", error["error"])
            self.assertEqual(question["id"], error["message_id"])
            self.assertEqual(0, main(shlex.split(error["recovery"]["show"])[1:]))
            self.assertEqual("Question?", json.loads(output.call_args.args[0])["body"])


class ProcessRecovery(unittest.TestCase):
    def test_late_answer_is_recovered_and_acked_by_fresh_cli_processes(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            db_path = path / "mail 'quoted' $draft.sqlite3"
            session_id = str(uuid.uuid4())
            environment = {**os.environ, "CODEX_THREAD_ID": session_id}

            def cli(*args, code=0, env=environment):
                run = subprocess.run([sys.executable, "-m", "harness_talk.cli", "--db", str(db_path), *args],
                                     capture_output=True, text=True, env=env, timeout=10)
                self.assertEqual(code, run.returncode, run.stdout + run.stderr)
                return json.loads(run.stdout)

            def recover(command, **kwargs):
                words = shlex.split(command)
                self.assertEqual(["htalk", "--db", str(db_path)], words[:3])
                return cli(*words[3:], **kwargs)

            # This nonexistent explicit socket prevents any real client discovery or notification.
            cli("peer", "add", "builder", "--harness", "codex", "--session", session_id,
                "--workspace", directory, "--socket", str(path / "absent.sock"))
            cli("peer", "add", "reviewer", "--harness", "claude", "--session", str(uuid.uuid4()),
                "--workspace", directory)
            question = cli("--as", "builder", "send", "reviewer", "--message", "Fixture question", "--no-notify")
            # The sender process has exited before this separate process saves the late answer.
            answer = cli("--as", "reviewer", "reply", question["id"], "--message", "Fixture late answer", code=2)
            self.assertEqual("not_submitted", answer["submission"])
            self.assertIsNotNone(answer["notification_started_at"])
            notification = {k: v for k, v in answer.items() if k.startswith("notification_") or k == "submission"}

            recovered = cli("--as", "builder", "sent")["messages"][0]
            self.assertEqual(question["id"], recovered["id"])
            shown = recover(recovered["recovery"]["show"])
            self.assertEqual(answer["id"], shown["reply"]["id"])
            self.assertEqual("reply_received", cli("--as", "builder", "wait", question["id"], "--seconds", "0")["state"])
            read = recover(recovered["recovery"]["show_reply"])
            self.assertEqual(question["id"], read["in_reply_to"])
            self.assertIsNone(read["ack_at"])
            self.assertIsNotNone(recover(read["recovery"]["ack_after_reading"])["ack_at"])
            self.assertEqual([], cli("--as", "builder", "inbox")["messages"])
            retry = cli("--as", "reviewer", "reply", question["id"], "--message", "Fixture late answer")
            self.assertEqual(answer["id"], retry["id"])
            self.assertFalse(retry["created"])
            self.assertEqual(notification, {k: retry[k] for k in notification})

            other_session = str(uuid.uuid4())
            conflict = cli("--as", "builder", "inbox", code=2,
                           env={**environment, "CODEX_THREAD_ID": other_session})
            self.assertEqual("actor_conflicts_with_CODEX_THREAD_ID", conflict["error"])
            self.assertEqual(session_id, conflict["registered_session_id"])
            self.assertEqual(other_session, conflict["current_session_id"])
            self.assertIn("return to the registered session", conflict["next_action"])
            self.assertEqual(2, len(recover(conflict["recovery"]["peers"])["peers"]))
            rebinding = cli("peer", "add", "builder", "--harness", "codex", "--session", other_session,
                            "--workspace", directory, code=2)
            self.assertEqual(session_id, rebinding["registered_session_id"])


class ClaudeAdapter(unittest.TestCase):
    def test_discovery_failure_and_uncertain_write_are_distinct(self):
        peer = {"name": "receiver", "harness": "claude", "session_id": "test"}
        message = {"id": "message", "sender": "sender"}
        with patch.object(adapters, "claude_socket", side_effect=ValueError("offline")):
            self.assertEqual("not_submitted", adapters.notify(peer, message, Path("/tmp/db"))[0])
        with patch.object(adapters, "claude_socket", return_value="/socket"), patch.object(adapters.socket, "socket") as socket:
            connection = socket.return_value.__enter__.return_value
            connection.sendall.side_effect = TimeoutError()
            self.assertEqual("submission_unknown", adapters.notify(peer, message, Path("/tmp/db"))[0])
            frame = json.loads(connection.sendall.call_args.args[0])
            self.assertEqual("htalk:sender", frame["from"])
            self.assertNotIn("permission", frame)


class CodexAdapter(unittest.TestCase):
    def test_exact_identity_and_loaded_status(self):
        peer = {"session_id": "test", "workspace": "/workspace"}
        for thread in ({"id": "other", "cwd": "/workspace", "status": {"type": "idle"}},
                       {"id": "test", "cwd": "/other", "status": {"type": "idle"}},
                       {"id": "test", "cwd": "/workspace", "status": {"type": "notLoaded"}}):
            rpc = unittest.mock.Mock()
            rpc.call.return_value = {"thread": thread}
            with self.assertRaises(ValueError):
                adapters.check_codex(rpc, peer)

    def test_rejection_after_queue_attempt_is_uncertain(self):
        peer = {"name": "receiver", "harness": "codex", "session_id": "test", "socket": "/socket"}
        rpc = unittest.mock.Mock()
        rpc.call.side_effect = TimeoutError()
        with patch.object(adapters, "codex_rpc") as connect, patch.object(adapters, "check_codex"):
            connect.return_value.__enter__.return_value = rpc
            outcome = adapters.notify(peer, {"id": "message", "sender": "sender"}, Path("/tmp/db"))
        self.assertEqual("submission_unknown", outcome[0])


if __name__ == "__main__":
    unittest.main()

@unittest.skipUnless(os.environ.get("HTALK_SOCKET_TESTS") == "1", "set HTALK_SOCKET_TESTS=1 when local socket binding is permitted")
class LocalSocketIntegration(unittest.TestCase):
    def test_codex_websocket_identity_and_queue_receipt(self):
        from websockets.sync.server import unix_serve
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            session = str(uuid.uuid4())
            methods = []
            def handler(connection):
                for raw in connection:
                    frame = json.loads(raw)
                    methods.append(frame["method"])
                    if frame["method"] == "initialized":
                        continue
                    if frame["method"] == "initialize":
                        self.assertTrue(frame["params"]["capabilities"]["experimentalApi"])
                        result = {}
                    elif frame["method"] == "thread/read":
                        self.assertEqual(session, frame["params"]["threadId"])
                        self.assertFalse(frame["params"]["includeTurns"])
                        result = {"thread": {"id": session, "cwd": directory, "status": {"type": "idle"}}}
                    else:
                        self.assertEqual("thread/queue/add", frame["method"])
                        result = {"queuedSubmission": {"id": "queue-receipt", "clientUserMessageId": frame["params"]["clientUserMessageId"]}}
                    connection.send(json.dumps({"id": frame["id"], "result": result}))
            with unix_serve(handler, str(path / "server.sock"), compression=None) as server:
                thread = threading.Thread(target=server.serve_forever)
                thread.start()
                try:
                    peer = {"name": "receiver", "harness": "codex", "session_id": session,
                            "workspace": directory, "socket": str(path / "server.sock")}
                    result = adapters.notify(peer, {"id": str(uuid.uuid4()), "sender": "sender"}, path / "db")
                    self.assertEqual(("submitted", "codex_queued:queue-receipt"), result)
                finally:
                    server.shutdown()
                    thread.join()
            self.assertEqual(["initialize", "initialized", "thread/read", "thread/queue/add"], methods)


class NativeCodexAdapter(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.home = Path(self.tmp.name)
        environment = patch.dict(os.environ, {"CODEX_HOME": str(self.home), "CODEX_SQLITE_HOME": ""})
        environment.start()
        self.addCleanup(environment.stop)
        self.peer = {"name": "receiver", "harness": "codex", "session_id": str(uuid.uuid4()),
                     "workspace": str(self.home), "socket": None}
        self.message = {"id": str(uuid.uuid4()), "sender": "sender"}
        self.state = self.home / "state_5.sqlite"
        with closing(sqlite3.connect(self.state)) as db, db:
            db.execute("CREATE TABLE threads (id TEXT PRIMARY KEY, cwd TEXT, archived INTEGER, source TEXT)")
            db.execute("INSERT INTO threads VALUES (?, ?, 0, 'cli')", (self.peer["session_id"], str(self.home)))

    def test_registration_and_saved_identity_need_no_socket_or_client_process(self):
        store = Store(self.home / "mail.sqlite3")
        self.assertIsNone(store.add_peer("receiver", "codex", self.peer["session_id"], self.home)["socket"])
        with patch.object(adapters.subprocess, "run") as run:
            result = adapters.probe(self.peer)
        run.assert_not_called()
        self.assertEqual(self.peer["session_id"], result["session_id"])
        self.assertEqual("unknown", result["runtime_status"])
        self.assertEqual("codex_cli_queue", result["transport"])

    def test_native_queue_uses_exact_uuid_and_usual_settings(self):
        queue_id = str(uuid.uuid4())
        receipt = f"Queued message {queue_id} for thread {self.peer['session_id']}.\n"
        with patch.object(adapters.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, receipt, "")) as run:
            outcome = adapters.notify(self.peer, self.message, self.home / "mail.sqlite3")
        self.assertEqual(("submitted", "codex_cli_queued:" + queue_id), outcome)
        run.assert_called_once_with(["codex", "queue", "--thread", self.peer["session_id"], "--message",
                                     adapters.notification(self.peer, self.message, self.home / "mail.sqlite3")],
                                    capture_output=True, text=True, timeout=20)

    def test_missing_changed_archived_or_non_cli_recipient_never_queues(self):
        for values in (("/other", 0, "cli"), (str(self.home), 1, "cli"), (str(self.home), 0, "exec")):
            with closing(sqlite3.connect(self.state)) as db, db:
                db.execute("UPDATE threads SET cwd=?, archived=?, source=?", values)
            with patch.object(adapters.subprocess, "run") as run:
                self.assertEqual("not_submitted", adapters.notify(self.peer, self.message, self.home / "mail.sqlite3")[0])
                run.assert_not_called()
        self.state.unlink()
        with patch.object(adapters.subprocess, "run") as run:
            self.assertEqual("not_submitted", adapters.notify(self.peer, self.message, self.home / "mail.sqlite3")[0])
            run.assert_not_called()
        self.assertFalse(self.state.exists())

    def test_unconfirmed_native_attempt_is_never_replayed(self):
        store = Store(self.home / "mail.sqlite3")
        store.add_peer("sender", "claude", str(uuid.uuid4()), self.home)
        store.add_peer("receiver", "codex", self.peer["session_id"], self.home)
        message, _ = store.save("sender", "receiver", "Question")
        with patch.object(adapters.subprocess, "run", side_effect=subprocess.TimeoutExpired("codex", 20)) as run:
            result = store.notify_once(message["id"], adapters.notify)
            store.notify_once(message["id"], adapters.notify)
        self.assertEqual("submission_unknown", result["submission"])
        run.assert_called_once()

    def test_failed_or_mismatched_cli_receipts_remain_uncertain(self):
        good = f"Queued message {uuid.uuid4()} for thread {self.peer['session_id']}.\n"
        for code, output in ((1, good), (0, "unexpected output"),
                             (0, f"Queued message {uuid.uuid4()} for thread {uuid.uuid4()}.\n")):
            with patch.object(adapters.subprocess, "run", return_value=subprocess.CompletedProcess([], code, output, "")):
                self.assertEqual("submission_unknown", adapters.notify(self.peer, self.message, self.home / "mail.sqlite3")[0])

    def test_user_sqlite_home_precedes_environment(self):
        configured = self.home / "configured"
        configured.mkdir()
        self.state.rename(configured / "state_5.sqlite")
        (self.home / "config.toml").write_text("sqlite_home = " + json.dumps(str(configured)))
        with patch.dict(os.environ, {"CODEX_SQLITE_HOME": str(self.home / "wrong")}):
            self.assertEqual(str(configured / "state_5.sqlite"), adapters.probe(self.peer)["metadata_source"])
