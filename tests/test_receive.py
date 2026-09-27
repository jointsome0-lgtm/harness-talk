"""Reconnect and crash ambiguity at the public native receiver interface."""
import json
from pathlib import Path
import sys
import uuid
from compat_support import HtalkCase, wait_for


class NativeReceiver(HtalkCase):
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
