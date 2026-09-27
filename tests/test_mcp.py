"""Public stdio interface: mailbox semantics and cleanup across client loss."""
import json
from pathlib import Path
import queue
import signal
import subprocess
import sys
import threading
import unittest
import uuid

from compat_support import HtalkCase, wait_for


class McpClient:
    def __init__(self, case, connector=None):
        self.process = subprocess.Popen(
            (case.argv(["mcp", "--connect", "--", *connector], False) if connector
             else case.argv(["--as", "alice", "mcp"], True)), cwd=case.tmp,
            env=case.environment(), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True)
        case.processes.append(self.process)
        self.responses = queue.Queue()
        self.sequence = 0
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()
        case.addCleanup(self.dispose)
        self.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                   "clientInfo": {"name": "htalk-test", "version": "1"}})
        self.send("notifications/initialized")

    def read(self):
        for line in self.process.stdout:
            self.responses.put(json.loads(line))

    def send(self, method, params=None, request_id=None):
        frame = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            frame["params"] = params
        if request_id is not None:
            frame["id"] = request_id
        self.process.stdin.write(json.dumps(frame) + "\n")
        self.process.stdin.flush()

    def request(self, method, params=None):
        self.sequence += 1
        self.send(method, params, self.sequence)
        while True:
            response = self.responses.get(timeout=15)
            if response.get("id") == self.sequence:
                return response

    def call(self, *args):
        return self.request("tools/call", {"name": "htalk", "arguments": {"args": list(args)}})["result"]

    def close(self):
        self.process.stdin.close()
        self.process.wait(timeout=15)
        assert self.process.returncode == 0, self.process.stderr.read()

    def dispose(self):
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait(timeout=5)
        self.reader.join(timeout=5)
        for stream in (self.process.stdin, self.process.stdout, self.process.stderr):
            stream.close()


class MailboxMcp(HtalkCase):
    def setUp(self):
        super().setUp()
        for peer in ("alice", "bob"):
            self.htalk("peer", "add", peer, "--harness", "generic", "--delivery", "pull")

    def test_exchange_scope_and_saved_errors(self):
        client = McpClient(self)
        tools = client.request("tools/list")["result"]["tools"]
        self.assertEqual(["htalk"], [tool["name"] for tool in tools])
        schema = tools[0]["inputSchema"]
        self.assertEqual(["args"], schema["required"])
        self.assertEqual(["args"], list(schema["properties"]))
        self.assertFalse(schema["additionalProperties"])
        for metadata in (True, False):
            result = client.request("tools/call", {"name": "htalk", "arguments": {
                "args": ["inbox"], "wait_for_previous": metadata}})["result"]
            self.assertFalse(result.get("isError", False))
        for extra in ({"wait_for_previous": "true"}, {"wait_for_previous": None}, {"db": "other.db"}):
            result = client.request("tools/call", {"name": "htalk", "arguments": {
                "args": ["inbox"], **extra}})["result"]
            self.assertTrue(result["isError"])
        self.assertFalse(client.call("send", "--help").get("isError", False))
        request_id = str(uuid.uuid4())
        for _ in range(2):
            result = client.call("send", "bob", "--id", request_id, "--message", "Check 7*8")
            self.assertFalse(result.get("isError", False))
            self.assertEqual(0, result["structuredContent"]["exit_code"])
            self.assertEqual(request_id, result["structuredContent"]["result"]["id"])
        self.assertEqual(1, self.sql("SELECT count(*) FROM messages")[0][0])
        self.htalk("--as", "bob", "ack", request_id)
        reply = self.htalk("--as", "bob", "reply", request_id, "--message", "56")
        shown = client.call("show", request_id)["structuredContent"]["result"]
        self.assertEqual("56", shown["reply"]["body"])
        self.assertIsNone(shown["reply"]["ack_at"])
        client.call("ack", reply["id"])
        self.assertIsNotNone(self.htalk("--as", "bob", "show", reply["id"])["ack_at"])
        for args in (["--as", "bob", "inbox"], ["--db", str(self.tmp / "other.db"), "inbox"],
                     ["peer", "retire", "bob"], ["watch"], ["mcp"],
                     ["send", "bob", "--message", "Missing retry identity"],
                     ["send", "bob", "--id", str(uuid.uuid4()), "--message-file", "/dev/stdin"]):
            self.assertTrue(client.call(*args)["isError"])
        missing = client.call("show", str(uuid.uuid4()))
        self.assertTrue(missing["isError"])
        self.assertEqual(2, missing["structuredContent"]["exit_code"])
        self.assertEqual("error", missing["structuredContent"]["result"]["state"])
        self.assertEqual(2, self.sql("SELECT count(*) FROM messages")[0][0])
        client.close()

    def test_remote_reconnect_and_lost_write_receipt_do_not_replay(self):
        mode = self.tmp / "link-mode"
        mode.write_text("offline")
        attempts = self.tmp / "attempts"
        connector = self.tmp / "connector.py"
        connector.write_text("""import json, os, subprocess, sys
from pathlib import Path
mode = Path(sys.argv[1]).read_text()
command = json.loads(sys.argv[3])
if mode == 'offline':
    sys.exit(1)
if mode == 'online':
    os.execv(command[0], command + ['mcp'])
for line in sys.stdin:
    frame = json.loads(line)
    if frame['method'] == 'initialize':
        result = {'protocolVersion': frame['params']['protocolVersion'],
                  'capabilities': {'tools': {}},
                  'serverInfo': {'name': 'harness-talk', 'version': 'test'}}
        print(json.dumps({'jsonrpc': '2.0', 'id': frame['id'], 'result': result}), flush=True)
    elif frame['method'] == 'tools/call':
        with open(sys.argv[2], 'a') as out:
            out.write(json.dumps(frame['params']) + '\\n')
        subprocess.run(command + frame['params']['arguments']['args'],
                       stdout=subprocess.DEVNULL, check=True)
        sys.exit(0)  # The write committed but its response never reached the client.
""")
        client = McpClient(self, [sys.executable, str(connector), str(mode), str(attempts),
                                  json.dumps(self.argv(["--as", "alice"], True))])
        self.assertTrue(client.call("inbox")["isError"])
        self.assertIsNone(client.process.poll())
        mode.write_text("drop")
        request_id = str(uuid.uuid4())
        sent = client.call("send", "bob", "--id", request_id, "--message", "one operation")
        self.assertTrue(sent["isError"])
        self.assertIn("outcome is unknown", sent["content"][0]["text"])
        self.assertEqual(1, len(attempts.read_text().splitlines()))
        mode.write_text("online")
        shown = client.call("show", request_id)["structuredContent"]["result"]
        self.assertEqual(request_id, shown["id"])
        self.assertEqual("one operation", shown["body"])
        client.call("send", "bob", "--id", request_id, "--message", "one operation")
        self.assertEqual(1, self.sql("SELECT count(*) FROM messages")[0][0])
        self.assertTrue(client.call("--as", "bob", "inbox")["isError"])
        client.close()

    def test_cancel_disconnect_and_shutdown_keep_saved_identity_and_reap_child(self):
        for remote, stop in [(remote, stop) for remote in (False, True)
                             for stop in ("cancel", "eof", "terminate")]:
            with self.subTest(remote=remote, stop=stop):
                client = McpClient(self, self.argv(["--as", "alice", "mcp"], True) if remote else None)
                request_id = str(uuid.uuid4())
                client.send("tools/call", {"name": "htalk", "arguments": {"args": [
                    "send", "bob", "--id", request_id, "--message", stop, "--wait", "45"]}}, 100)
                wait_for(lambda: self.sql("SELECT id FROM messages WHERE id=?", (request_id,)))
                children_file = Path(f"/proc/{client.process.pid}/task/{client.process.pid}/children")
                children = wait_for(lambda: children_file.read_text().split())
                if remote:
                    endpoint = children[0]
                    nested = Path(f"/proc/{endpoint}/task/{endpoint}/children")
                    children += wait_for(lambda: nested.read_text().split())
                self.assertIn("result", client.request("ping"))
                if stop == "cancel":
                    client.process.send_signal(signal.SIGINT)
                    self.assertIn("result", client.request("ping"))
                    client.send("notifications/cancelled", {"requestId": 100, "reason": "test"})
                elif stop == "eof":
                    client.close()
                else:
                    client.process.send_signal(signal.SIGTERM)
                    client.process.wait(timeout=15)  # stdin remains open
                    self.assertEqual(0, client.process.returncode)
                wait_for(lambda: all(not Path(f"/proc/{pid}").exists() for pid in children))
                if stop == "cancel":
                    retry = client.call("send", "bob", "--id", request_id, "--message", stop)
                    self.assertEqual(request_id, retry["structuredContent"]["result"]["id"])
                    client.close()
                self.assertEqual(1, self.sql("SELECT count(*) FROM messages WHERE id=?", (request_id,))[0][0])


if __name__ == "__main__":
    unittest.main()
