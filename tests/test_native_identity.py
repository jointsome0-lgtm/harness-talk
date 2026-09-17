from functools import partial
import json
import os
from pathlib import Path
import shlex
import tempfile
import unittest
import uuid
from unittest.mock import patch

from harness_talk import identity
from harness_talk.cli import main
from harness_talk.store import Store


class NativeClaudeIdentity(unittest.TestCase):
    """Uses a synthetic /proc tree and sessions directory, never the running system's."""

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.path = Path(temporary.name)
        self.proc, self.sessions = self.path / "proc", self.path / "sessions"
        self.sessions.mkdir()
        self.session_id = str(uuid.uuid4())
        # htalk (40) runs in a tool shell (30) of Claude Code (20), started from a terminal shell (10).
        self.process(1, 0, "systemd", exe="/usr/lib/systemd/systemd")
        self.process(10, 1, "bash")
        self.process(20, 10, "claude", start=4242, exe="/home/user/.local/share/claude/versions/2.1.274")
        self.process(30, 20, "bash")
        self.process(40, 30, "htalk", exe="/usr/bin/python3.14")
        self.metadata(procStart="4242")
        self.environment = {"CLAUDE_CODE_SESSION_ID": self.session_id, "CLAUDE_PID": "20"}
        self.store = Store(self.path / "mail.sqlite3")

    def process(self, pid, parent, comm, start=100, exe="/usr/bin/bash"):
        directory = self.proc / str(pid)
        directory.mkdir(parents=True)
        fields = ["S", str(parent), *["0"] * 17, str(start), *["0"] * 32]
        (directory / "stat").write_text(f"{pid} ({comm}) {' '.join(fields)}\n")
        (directory / "exe").symlink_to(exe)

    def metadata(self, **changes):
        saved = {"pid": 20, "sessionId": self.session_id, "procStart": "4242", "cwd": str(self.path),
                 "messagingSocketPath": "/unused.sock", **changes}
        (self.sessions / "20.json").write_text(json.dumps(saved))

    def detect(self, pid=40, **environment):
        return identity.claude_session({**self.environment, **environment}, self.proc, self.sessions, pid)

    def cli(self, *words, code=0, pid=40, **environment):
        detect = partial(identity.claude_session, proc=self.proc, sessions=self.sessions, pid=pid)
        with patch.dict(os.environ, environment, clear=True), patch("harness_talk.cli.claude_session", detect), \
                patch("builtins.print") as output:
            self.assertEqual(code, main(["--db", str(self.store.path), *words]))
        return json.loads(output.call_args.args[0])

    def test_recognizes_the_claude_session_above_the_command(self):
        recognized = {"harness": "claude", "status": "recognized", "session_id": self.session_id, "workspace": str(self.path)}
        self.assertEqual(recognized, self.detect())
        self.metadata(procStart=4242)
        self.assertEqual(recognized, self.detect())
        # Process names may contain spaces and parentheses.
        self.process(50, 20, "tool) (1 2", exe="/usr/bin/bash")
        self.process(51, 50, "htalk", exe="/usr/bin/python3.14")
        self.assertEqual("recognized", self.detect(pid=51)["status"])

    def test_refuses_commands_outside_the_claude_process_tree(self):
        # A tmux server started by a Claude command detaches, so its panes descend from init.
        self.process(60, 1, "tmux: server")
        self.process(61, 60, "bash")
        self.process(62, 61, "htalk", exe="/usr/bin/python3.14")
        self.assertEqual("claude_pid_not_an_ancestor", self.detect(pid=62)["reason"])
        parent = 20
        for pid in range(100, 100 + identity.MAX_DEPTH):
            self.process(pid, parent, "bash")
            parent = pid
        self.process(200, parent, "htalk")
        self.assertEqual("recognized", self.detect(pid=200)["status"])
        self.process(201, 200, "htalk")
        self.assertEqual("claude_pid_not_an_ancestor", self.detect(pid=201)["reason"])

    def test_refuses_commands_of_clients_nested_in_claude(self):
        self.assertEqual("codex_thread_id_present", self.detect(CODEX_THREAD_ID=str(uuid.uuid4()))["reason"])
        # The kernel truncates comm to 15 bytes, and a client may run under another process name.
        clients = (("codex-code-mode", "/usr/bin/node"), ("MainThread", "/opt/codex/codex-x86_64-unknown-linux-musl"),
                   ("opencode", "/usr/bin/bun"), ("claude", "/home/user/.local/share/claude/versions/2.1.274"))
        for pid, (comm, exe) in zip(range(300, 400, 10), clients):
            with self.subTest(comm=comm, exe=exe):
                self.process(pid, 30, comm, exe=exe)
                self.process(pid + 1, pid, "bash")
                self.process(pid + 2, pid + 1, "htalk")
                self.assertEqual("nested_client_process", self.detect(pid=pid + 2)["reason"])

    def test_refuses_absent_or_inconsistent_session_evidence(self):
        self.assertEqual("claude_session_variables_absent", identity.claude_session({}, self.proc, self.sessions, 40)["reason"])
        for environment in ({"CLAUDE_PID": "20x"}, {"CLAUDE_PID": "²0"}, {"CLAUDE_CODE_SESSION_ID": self.session_id.upper()}):
            self.assertEqual("claude_session_variables_invalid", self.detect(**environment)["reason"])
        self.assertEqual("claude_process_unavailable", self.detect(CLAUDE_PID="21")["reason"])
        self.assertEqual("claude_process_unavailable", identity.claude_session(self.environment, self.path / "absent", self.sessions, 40)["reason"])
        for changes in ({"procStart": "4243"}, {"sessionId": str(uuid.uuid4())}, {"pid": 21}, {"pid": "20"}, {"procStart": None}):
            with self.subTest(changes=changes):
                self.metadata(**changes)
                self.assertEqual("claude_session_metadata_mismatch", self.detect()["reason"])
        self.metadata()
        self.process(80, 999, "htalk")
        self.assertEqual("process_ancestry_unavailable", self.detect(pid=80)["reason"])
        (self.proc / "30" / "exe").unlink()
        self.assertEqual("process_ancestry_unavailable", self.detect()["reason"])
        (self.sessions / "20.json").write_text("[]")
        self.assertEqual("claude_session_metadata_mismatch", self.detect()["reason"])
        (self.sessions / "20.json").write_text("{")
        self.assertEqual("claude_session_metadata_unavailable", self.detect()["reason"])
        (self.sessions / "20.json").unlink()
        self.assertEqual("claude_session_metadata_unavailable", self.detect()["reason"])

    def test_registered_session_can_omit_its_name(self):
        self.store.add_peer("reviewer", "claude", self.session_id, self.path)
        self.store.add_peer("builder", "claude", str(uuid.uuid4()), self.path)
        question = self.cli("send", "builder", "--message", "Question", "--no-notify", **self.environment)
        self.assertEqual(("reviewer", "native_session"), (question["sender"], question["actor_source"]))
        show = shlex.split(question["recovery"]["show"])
        self.assertEqual(["--as", "reviewer"], show[3:5])
        self.assertEqual("option", self.cli(*show[3:], **self.environment)["actor_source"])
        with patch("harness_talk.cli.notify", side_effect=KeyboardInterrupt()):
            interrupted = self.cli("send", "builder", "--message", "Interrupted", code=130, **self.environment)
        self.assertEqual(("saved", "native_session"), (interrupted["persistence"], interrupted["actor_source"]))
        self.assertIn("--as reviewer", interrupted["recovery"]["show"])
        self.assertEqual("HTALK_PEER", self.cli("inbox", HTALK_PEER="builder")["actor_source"])
        self.assertEqual("option", self.cli("--as", "reviewer", "inbox", HTALK_PEER="builder", **self.environment)["actor_source"])

    def test_explicit_claude_name_must_match_a_recognized_session(self):
        self.store.add_peer("reviewer", "claude", self.session_id, self.path)
        builder = self.store.add_peer("builder", "claude", str(uuid.uuid4()), self.path)
        self.store.add_peer("alice", "codex", str(uuid.uuid4()), self.path)
        for words, extra, source in ((["--as", "builder"], {}, "option"), ([], {"HTALK_PEER": "builder"}, "HTALK_PEER")):
            with self.subTest(source=source):
                conflict = self.cli(*words, "inbox", code=2, **self.environment, **extra)
                self.assertEqual("actor_conflicts_with_CLAUDE_CODE_SESSION_ID", conflict["error"])
                self.assertEqual((builder["session_id"], self.session_id, source),
                                 (conflict["registered_session_id"], conflict["current_session_id"], conflict["actor_source"]))
                self.assertIn("peer list", conflict["recovery"]["peers"])
                self.assertIn("--as builder", conflict["recovery"]["inbox_in_registered_session"])
        # Without process evidence, or for a different client's peer, an explicit name works as before.
        self.assertEqual("option", self.cli("--as", "builder", "inbox", pid=62, **self.environment)["actor_source"])
        (self.sessions / "20.json").unlink()
        self.assertEqual("option", self.cli("--as", "builder", "inbox", **self.environment)["actor_source"])
        self.assertEqual("option", self.cli("--as", "alice", "inbox", **self.environment)["actor_source"])

    def test_commands_without_a_name_explain_the_native_session(self):
        unregistered = self.cli("inbox", code=2, **self.environment)
        self.assertEqual("peer_required_use_as_or_HTALK_PEER", unregistered["error"])
        self.assertEqual({"harness": "claude", "status": "recognized", "session_id": self.session_id, "workspace": str(self.path)},
                         unregistered["native_session"])
        self.assertIn("peer add", unregistered["next_action"])
        self.assertIn("peer list", unregistered["recovery"]["peers"])
        # A client nested in the registered Claude session does not inherit its name.
        self.store.add_peer("reviewer", "claude", self.session_id, self.path)
        nested = self.cli("inbox", code=2, CODEX_THREAD_ID=str(uuid.uuid4()), **self.environment)
        self.assertEqual({"harness": "claude", "status": "unrecognized", "reason": "codex_thread_id_present"}, nested["native_session"])
        self.assertIn("--as NAME", nested["next_action"])
        self.process(70, 30, "codex", exe="/opt/codex/codex")
        self.process(71, 70, "htalk")
        self.assertEqual("nested_client_process", self.cli("inbox", code=2, pid=71, **self.environment)["native_session"]["reason"])
        self.assertEqual("claude_session_variables_absent", self.cli("inbox", code=2)["native_session"]["reason"])


if __name__ == "__main__":
    unittest.main()
