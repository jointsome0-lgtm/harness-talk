"""Reconnect and crash ambiguity at the public native receiver interface."""
import json
from pathlib import Path
import shutil
import sys
import tomllib
import uuid
from compat_support import HtalkCase, wait_for


class NativeReceiver(HtalkCase):
    def test_current_mail_is_checked_and_unanswered_ack_is_still_work(self):
        peer = self.codex_recipient("alice")
        self.htalk("peer", "add", "bob", "--harness", "generic", "--delivery", "pull")
        self.configure(codex_queue={"queue_id": str(uuid.uuid4())})
        def send(sender, recipient):
            return self.htalk("--as", sender, "send", recipient,
                              "--message", "Check this", "--no-notify")
        answered, pending = send("bob", "alice"), send("bob", "alice")
        answers = [self.htalk("--as", "bob", "reply", send("alice", "bob")["id"],
                             "--message", "Done", "--no-notify") for _ in range(2)]
        messages = [answered, *answers, pending]
        events = self.tmp / "events"
        events.write_text("".join(json.dumps({"event": "message", "id": m["id"],
                                             "seq": m["seq"]}) + "\n" for m in messages))
        # The captured events become stale before the receiver consumes them.
        self.htalk("--as", "alice", "reply", answered["id"], "--message", "Finished", "--no-notify")
        self.htalk("--as", "alice", "ack", answers[0]["id"])
        self.htalk("--as", "alice", "ack", pending["id"])
        watch = self.tmp / "watch.py"
        watch.write_text("import sys\nfrom pathlib import Path\n"
                         "print('{\"event\":\"ready\",\"peer\":\"alice\"}', flush=True)\n"
                         "print(Path(sys.argv[1]).read_text(), end='', flush=True)\nsys.stdin.read()\n")
        mode, attempts = self.tmp / "mode", self.tmp / "attempts"
        mode.write_text("offline")
        mcp = self.tmp / "mcp"
        command = self.argv(["--as", "alice", "mcp"], True)
        mcp.write_text(f"#!{sys.executable}\nimport os, sys\nfrom pathlib import Path\n"
                       f"with open({str(attempts)!r}, 'a') as f: f.write(str(os.getpid())+'\\n')\n"
                       f"mode=Path({str(mode)!r}).read_text()\n"
                       "if mode == 'offline': sys.exit(1)\n"
                       "if mode == 'hang': sys.stdin.read(); sys.exit(1)\n"
                       f"os.execv({command[0]!r}, {command!r})\n")
        mcp.chmod(0o700)
        state = self.tmp / "receiver"
        base = ["receive", "--peer", "alice", "--session", peer["session_id"],
                "--workspace", str(self.work), "--state", str(state)]
        words = [*base, "--mcp-command", str(mcp), "--", sys.executable, str(watch), str(events)]
        saved = lambda: json.loads((state / "state.json").read_text())
        process = self.spawn(*words, db=False)
        wait_for(lambda: attempts.exists() and len(attempts.read_text().splitlines()) >= 2)
        self.assertEqual([], self.calls("codex", ["queue"]))
        self.assertIsNone(saved()["pending"])
        mode.write_text("online")
        expected = {pending["id"], answers[1]["id"]}
        wait_for(lambda: expected == set(saved()["receipts"]))
        process.terminate()
        stdout, stderr = process.communicate(timeout=15)
        self.assertEqual(0, process.returncode, stderr)
        skipped = {v["id"] for line in stdout.splitlines()
                   if (v := json.loads(line))["event"] == "skipped"}
        self.assertEqual({answered["id"], answers[0]["id"]}, skipped)
        self.assertEqual(2, len(self.calls("codex", ["queue"])))
        self.assertIsNone(self.htalk("--as", "alice", "show", pending["id"])["reply"])
        self.assertIsNone(self.htalk("--as", "alice", "show", answers[1]["id"])["ack_at"])
        # Adding the first checker to a legacy receipt file preserves history.
        legacy = saved()
        legacy["binding"].pop("mcp_command")
        (state / "state.json").write_text(json.dumps(legacy))
        count = len(attempts.read_text().splitlines())
        process = self.spawn(*words, db=False)
        wait_for(lambda: len(attempts.read_text().splitlines()) > count)
        process.terminate()
        process.communicate(timeout=15)
        self.assertEqual(expected, set(saved()["receipts"]))
        self.assertEqual(str(mcp), saved()["binding"]["mcp_command"])
        blocked = self.run_raw(*base, "--", sys.executable, str(watch), str(events), db=False)
        self.assertEqual(2, blocked.code)
        self.assertIn("binding changed", blocked.stderr)
        # A valid show result for a different recipient must stop before waking.
        wrong = send("alice", "bob")
        events.write_text(json.dumps({"event": "message", "id": wrong["id"], "seq": wrong["seq"]}) + "\n")
        rejected = self.run_raw(*words, db=False)
        self.assertEqual(2, rejected.code)
        self.assertIn("differs from the watched message or recipient", rejected.stderr)
        self.assertIsNone(saved()["pending"])
        # Cancellation also cleans up a checker still waiting for its handshake.
        mode.write_text("hang")
        count = len(attempts.read_text().splitlines())
        process = self.spawn(*words, db=False)
        wait_for(lambda: len(attempts.read_text().splitlines()) > count)
        checker_pid = int(attempts.read_text().splitlines()[-1])
        process.terminate()
        process.communicate(timeout=15)
        self.assertEqual(0, process.returncode)
        self.assertFalse(Path(f"/proc/{checker_pid}").exists())
        self.assertEqual(2, len(self.calls("codex", ["queue"])))

    def test_store_move_stops_before_native_write_and_keeps_recovery(self):
        peer = self.codex_recipient("alice")
        self.configure(codex_queue={"queue_id": str(uuid.uuid4())})
        ready, gate = self.tmp / "ready", self.tmp / "release"
        message_id = str(uuid.uuid4())
        watch = self.tmp / "watch.py"
        watch.write_text(f"import json, sys, time\nfrom pathlib import Path\n"
                         f"Path({str(ready)!r}).touch()\n"
                         "print('{\"event\":\"ready\",\"peer\":\"alice\"}', flush=True)\n"
                         f"while not Path({str(gate)!r}).exists(): time.sleep(0.01)\n"
                         f"print({json.dumps({'event':'message','id':message_id,'seq':1})!r}, flush=True)\n"
                         "sys.stdin.read()\n")
        state = self.tmp / "receiver"
        words = ["receive", "--peer", "alice", "--session", peer["session_id"],
                 "--workspace", str(self.work), "--state", str(state), "--", sys.executable, str(watch)]
        process = self.spawn(*words, db=False)
        wait_for(ready.exists)
        other = self.tmp / "relocated-store"
        other.mkdir()
        shutil.copyfile(self.codex_home / "state_5.sqlite", other / "state_5.sqlite")
        config = self.codex_home / "config.toml"
        config.write_text("sqlite_home = " + json.dumps(str(other)))
        gate.touch()
        stdout, stderr = process.communicate(timeout=15)
        self.assertEqual(2, process.returncode, stderr)
        self.assertIn("codex_store_changed", stdout)
        self.assertEqual([], self.calls("codex", ["queue"]))
        saved = lambda: json.loads((state / "state.json").read_text())
        self.assertIsNone(saved()["pending"])
        config.unlink()
        process = self.spawn(*words, db=False)
        wait_for(lambda: message_id in saved()["receipts"])
        process.terminate()
        process.communicate(timeout=15)
        calls = self.calls("codex", ["queue"])
        self.assertEqual(1, len(calls))
        pinned = calls[0]["argv"][calls[0]["argv"].index("--config") + 1]
        self.assertEqual(str(self.codex_home), tomllib.loads(pinned)["sqlite_home"])
        # A file symlink may point to a different basename. Its parent alone
        # would select another store, possibly one owned by another receiver.
        renamed = other / "renamed.sqlite"
        (self.codex_home / "state_5.sqlite").rename(renamed)
        (self.codex_home / "state_5.sqlite").symlink_to(renamed)
        alias_words = words.copy()
        alias_words[alias_words.index("--state") + 1] = str(self.tmp / "file-alias-state")
        process = self.spawn(*alias_words, db=False)
        wait_for(lambda: process.poll() is not None or len(self.calls("codex", ["queue"])) > 1)
        if process.poll() is None:
            process.terminate()
        stdout, stderr = process.communicate(timeout=15)
        self.assertEqual(1, len(self.calls("codex", ["queue"])), self.calls("codex", ["queue"])[-1])
        self.assertEqual(2, process.returncode, stdout + stderr)
        self.assertIn("codex_store_name_unsupported", stdout)

    def test_session_ownership_uses_saved_store_and_survives_cancellation(self):
        alice = self.codex_recipient("alice")
        bob = self.codex_recipient("bob")
        self.configure(codex_queue={"mode": "block"})
        connector = self.tmp / "watch.py"
        connector.write_text("""import json, os, sys
from pathlib import Path
Path(sys.argv[3]).write_text(str(os.getpid()))
print(json.dumps({"event": "ready", "peer": sys.argv[1]}), flush=True)
print(json.dumps({"event": "message", "id": sys.argv[2], "seq": 1}), flush=True)
sys.stdin.read()
""")
        def words(peer, state):
            return ["receive", "--peer", peer["name"], "--session", peer["session_id"],
                    "--workspace", str(self.work), "--state", str(self.tmp / state),
                    "--", sys.executable, str(connector), peer["name"], str(uuid.uuid4()),
                    str(self.tmp / (state + "-connector.pid"))]
        first_words = words(alice, "first")
        first = self.spawn(*first_words, db=False)
        self.started("codex_queue")
        # Another home may redirect to the same saved Codex store.
        alias = self.tmp / "other-codex-home"
        alias.mkdir()
        (alias / "config.toml").write_text("sqlite_home = " + json.dumps(str(self.codex_home)))
        other_words = words(alice, "other-state")
        blocked = self.run_raw(*other_words, db=False, env={"CODEX_HOME": str(alias)})
        self.assertEqual(2, blocked.code)
        self.assertIn("Another receiver owns this Codex session", blocked.stderr)
        self.assertEqual(1, len(self.calls("codex", ["queue"])))
        # A different session in the same store may run concurrently.
        second = self.spawn(*words(bob, "second"), db=False)
        wait_for(lambda: len(self.calls("codex", ["queue"])) == 2)
        first.terminate()
        connector_pid = int((self.tmp / "first-connector.pid").read_text())
        wait_for(lambda: not Path(f"/proc/{connector_pid}").exists())
        # Its native write is still blocked; ownership must not escape with
        # the cancelled async future while that write can still take effect.
        blocked = self.run_raw(*other_words, db=False)
        self.assertEqual(2, blocked.code)
        self.assertIn("Another receiver owns this Codex session", blocked.stderr)
        self.release("codex_queue")
        first.communicate(timeout=15)
        self.assertEqual(0, first.returncode)
        second.communicate(timeout=15)
        self.assertEqual(2, second.returncode)  # fake native write has no receipt
        stopped = self.run_raw(*first_words, db=False)
        self.assertEqual(2, stopped.code)
        self.assertIn("outcome unknown", stopped.stderr)

    def test_reconnect_deduplicates_and_uncertain_restart_stops(self):
        peer = self.codex_recipient("alice")
        self.configure(codex_queue={"queue_id": str(uuid.uuid4())})
        message_id, uncertain_id = str(uuid.uuid4()), str(uuid.uuid4())
        events = self.tmp / "events"
        connector = self.tmp / "watch.py"
        connections = self.tmp / "connections"
        connector.write_text("""from pathlib import Path
import sys
with open(sys.argv[2], 'a') as log:
    log.write('connected\\n')
if len(Path(sys.argv[2]).read_text().splitlines()) == 1:
    print('{\"event\":\"mess', end='', flush=True)
else:
    print(Path(sys.argv[1]).read_text(), flush=True)
""")
        def watch(message):
            events.write_text(json.dumps({"event": "ready", "peer": "alice"}) + "\n"
                + json.dumps({"event": "message", "id": message, "seq": 1,
                              "notification": "Untrusted text must not enter the native queue"}))
        watch(message_id)
        state = self.tmp / "receiver"
        words = ["receive", "--peer", "alice", "--session", peer["session_id"],
                 "--workspace", str(self.work), "--state", str(state), "--", sys.executable,
                 str(connector), str(events), str(connections)]
        process = self.spawn(*words, db=False)
        def saved():
            try:
                return json.loads((state / "state.json").read_text())
            except FileNotFoundError:
                return {}
        wait_for(lambda: message_id in saved().get("receipts", {}))
        wait_for(lambda: len(connections.read_text().splitlines()) >= 3)
        process.terminate()
        process.communicate(timeout=15)
        self.assertEqual(0, process.returncode)
        calls = self.calls("codex", ["queue"])
        self.assertEqual(1, len(calls))
        self.assertIn(message_id, calls[0]["argv"][-1])
        self.assertNotIn("Untrusted text", calls[0]["argv"][-1])
        # A normal receiver restart keeps the durable deduplication receipt.
        previous = len(connections.read_text().splitlines())
        process = self.spawn(*words, db=False)
        wait_for(lambda: len(connections.read_text().splitlines()) > previous)
        self.configure(codex_queue={"stdout": "Ambiguous queue result"})
        watch(uncertain_id)
        process.communicate(timeout=15)
        self.assertEqual(2, process.returncode)
        self.assertEqual(uncertain_id, saved()["pending"])
        self.assertEqual(2, len(self.calls("codex", ["queue"])))
        blocked = self.run_raw(*words, db=False)
        self.assertEqual(2, blocked.code)
        self.assertIn(uncertain_id, blocked.stderr)
        self.assertEqual(2, len(self.calls("codex", ["queue"])))
