"""Black-box harness for the htalk CLI compatibility suite.

Every check runs the selected htalk executable as a subprocess, with a private
home, fake claude/codex/opencode executables and an explicit --db in a temporary
directory. Nothing here imports the Python implementation.

Select the command with HTALK_TEST_COMMAND, a JSON list of executable arguments,
for example ["/path/to/python", "-m", "harness_talk"] or ["/path/to/htalk"].
Without it, the checkout's target/debug/htalk is used when it exists. An
installed htalk is never used, and a missing command fails every test.
"""
from contextlib import closing
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import shlex
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import uuid

REPO = Path(__file__).resolve().parents[1]
# Only interpreter selection passes through, so a Python command can find its source.
PASSTHROUGH = ("PYTHONPATH", "PYTHONDONTWRITEBYTECODE")
TIMEOUT = 30

PEER_KEYS = {"name", "harness", "session_id", "workspace", "socket", "url", "retired_at"}
ROW_KEYS = {"seq", "id", "sender", "recipient", "in_reply_to", "body", "created_at", "ack_at", "submission",
            "notification_started_at", "notification_finished_at", "notification_detail", "wait_returned_at"}
MESSAGE_KEYS = ROW_KEYS | {"reply", "state", "recovery", "next_action"}


class CommandUnavailable(Exception):
    pass


def htalk_command():
    raw = os.environ.get("HTALK_TEST_COMMAND")
    if raw:
        try:
            words = json.loads(raw)
        except ValueError:
            raise CommandUnavailable("HTALK_TEST_COMMAND must be a JSON list of strings") from None
        if not isinstance(words, list) or not words or not all(isinstance(w, str) and w for w in words):
            raise CommandUnavailable("HTALK_TEST_COMMAND must be a nonempty JSON list of strings")
        executable = Path(words[0])
        if not executable.is_absolute():
            raise CommandUnavailable("HTALK_TEST_COMMAND must start with an absolute executable path, "
                                     "not a name looked up on PATH: %r" % words[0])
        if not (executable.is_file() and os.access(executable, os.X_OK)):
            raise CommandUnavailable("HTALK_TEST_COMMAND executable is missing: %s" % executable)
        return words
    local = REPO / "target/debug/htalk"
    if local.is_file() and os.access(local, os.X_OK):
        return [str(local)]
    raise CommandUnavailable("No htalk under test: set HTALK_TEST_COMMAND or build %s" % local)


def process_start(pid):
    """The start time field of /proc/PID/stat, as Claude Code records it."""
    text = Path("/proc/%d/stat" % pid).read_text()
    return text.rpartition(")")[2].split()[19]


def wait_for(predicate, timeout=15, message="condition"):
    """Poll a condition. Only a generous upper bound is used; nothing asserts durations."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.02)
    raise AssertionError("timed out waiting for " + message)


# One script serves as claude, codex and opencode. It records each invocation and
# follows STATE/config.json. It never contacts a real client.
FAKE_CLIENT = r'''#!@PYTHON@ -B
import json, os, sys, time
CLIENT, STATE = @CLIENT@, @STATE@

def log(**entry):
    with open(os.path.join(STATE, "calls.jsonl"), "a") as out:
        out.write(json.dumps({"client": CLIENT, "pid": os.getpid(), **entry}) + "\n")

def mark(name):
    open(os.path.join(STATE, name), "w").close()

def gate(name):
    mark(name + ".started")
    deadline = time.monotonic() + 25
    while not os.path.exists(os.path.join(STATE, name + ".release")) and time.monotonic() < deadline:
        time.sleep(0.02)

try:
    with open(os.path.join(STATE, "config.json")) as source:
        config = json.load(source)
except FileNotFoundError:
    config = {}
argv = sys.argv[1:]
log(argv=argv)
if CLIENT == "claude" and argv == ["agents", "--json"]:
    settings = config.get("claude_agents", {})
    if settings.get("gate"):
        gate("claude_agents")
    sys.stdout.write(settings.get("stdout", json.dumps(settings.get("rows", []))))
    sys.exit(settings.get("exit", 0))
if CLIENT == "codex" and argv[:1] == ["queue"] and len(argv) == 5 and argv[1] == "--thread" and argv[3] == "--message":
    settings = config.get("codex_queue", {})
    mode = settings.get("mode", "ok")
    if mode == "block":
        gate("codex_queue")
        sys.exit(1)
    receipt = "Queued message %s for thread %s.\n" % (settings.get("queue_id"), argv[2])
    sys.stdout.write(settings.get("stdout", receipt))
    sys.exit(1 if mode == "fail" else 0)
if CLIENT == "codex" and argv == ["app-server", "--stdio"]:
    settings = config.get("codex_app_server", {})
    for line in sys.stdin:
        frame = json.loads(line)
        log(rpc=frame.get("method"), params=frame.get("params"))
        if "id" not in frame:
            continue
        if frame["method"] == "initialize":
            answer = {"id": frame["id"], "result": {}}
        elif frame["method"] == "thread/queue/delete":
            if settings.get("mode") == "block":
                gate("codex_delete")
                sys.exit(1)
            answer = {"id": frame["id"], "result": {"deleted": settings.get("deleted", True)}}
        else:
            answer = {"id": frame["id"], "error": {"code": -32601, "message": "unsupported by fake"}}
        sys.stdout.write(json.dumps(answer) + "\n")
        sys.stdout.flush()
    sys.exit(0)
sys.exit(64)
'''


class Result:
    def __init__(self, code, stdout, stderr):
        self.code, self.stdout, self.stderr = code, stdout, stderr
        try:
            self.json = json.loads(stdout)
        except ValueError:
            self.json = None


class ClaudeSocket:
    """A fake Claude Code messaging socket. Frames are collected after htalk exits:
    a connected client's bytes wait in the listen backlog, so no thread is needed."""
    def __init__(self, path):
        self.path = str(path)
        self.server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.server.bind(self.path)
        self.server.listen(16)
        self.server.setblocking(False)
        self.received = []

    def frames(self):
        while True:
            try:
                connection, _ = self.server.accept()
            except BlockingIOError:
                break
            connection.setblocking(True)
            connection.settimeout(5)
            data = b""
            with connection:
                while True:
                    chunk = connection.recv(65536)
                    if not chunk:
                        break
                    data += chunk
            self.received.extend(json.loads(line) for line in data.decode().splitlines() if line.strip())
        return list(self.received)

    def close(self):
        self.server.close()


class FakeOpenCode(BaseHTTPRequestHandler):
    """The documented OpenCode server routes htalk uses, with invented data."""
    def log_message(self, *args):
        pass

    def answer(self, status, data=None):
        body = b"" if data is None else json.dumps(data).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def handle_request(self, method):
        state = self.server.state
        path, _, query = self.path.partition("?")
        length = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(length) or b"null")
        state["requests"].append({"method": method, "path": path, "query": query, "body": body,
                                  "authorization": self.headers.get("Authorization")})
        parts = path.split("/")
        if (method, path) == ("GET", "/global/health"):
            return self.answer(200, {"healthy": True, "version": "1.18.30"})
        if (method, path) == ("GET", "/session/status"):
            return self.answer(200, {})
        session = state["sessions"].get(parts[2]) if len(parts) > 2 else None
        if session is None:
            return self.answer(404, {"name": "NotFoundError"})
        if method == "GET" and len(parts) == 3:
            return self.answer(200, session)
        if method == "POST" and parts[3:] == ["prompt_async"]:
            return self.answer(204)
        return self.answer(404, {"name": "NotFoundError"})

    def do_GET(self):
        self.handle_request("GET")

    def do_POST(self):
        self.handle_request("POST")


class HtalkCase(unittest.TestCase):
    """A private environment per test: home, client homes, fake clients and database."""
    maxDiff = None

    def setUp(self):
        try:
            self.command = htalk_command()
        except CommandUnavailable as exc:
            self.fail(str(exc))
        temporary = tempfile.TemporaryDirectory(prefix="htc")
        self.addCleanup(temporary.cleanup)
        self.tmp = Path(temporary.name).resolve()
        self.home = self.tmp / "home"
        (self.home / ".claude/sessions").mkdir(parents=True)
        self.codex_home = self.tmp / "codex-home"
        self.codex_home.mkdir()
        self.work = self.tmp / "work"
        self.work.mkdir()
        self.state = self.tmp / "fake"
        self.state.mkdir()
        self.bin = self.tmp / "bin"
        self.bin.mkdir()
        for client in ("claude", "codex", "opencode"):
            script = self.bin / client
            script.write_text(FAKE_CLIENT.replace("@PYTHON@", sys.executable).replace("@CLIENT@", repr(client))
                              .replace("@STATE@", repr(str(self.state))))
            script.chmod(0o755)
        self.config = {}
        self.claude_rows = []
        self.sockets = []
        self.processes = []
        self.db = self.tmp / "data" / "mail.sqlite3"
        self.addCleanup(self.stop_leftovers)

    # Environment and invocation

    def environment(self, extra=None):
        env = {"PATH": "%s:/usr/bin:/bin" % self.bin, "HOME": str(self.home), "CODEX_HOME": str(self.codex_home),
               "LANG": "C.UTF-8", "LC_ALL": "C.UTF-8", "TMPDIR": str(self.tmp)}
        env.update({key: os.environ[key] for key in PASSTHROUGH if key in os.environ})
        env.update(extra or {})
        return {key: value for key, value in env.items() if value is not None}

    def argv(self, words, db):
        return [*self.command, *(["--db", str(self.db if db is True else db)] if db else []), *map(str, words)]

    def run_raw(self, *words, db=True, env=None, cwd=None):
        completed = subprocess.run(self.argv(words, db), capture_output=True, text=True, encoding="utf-8",
                                   env=self.environment(env), cwd=cwd or self.tmp, timeout=TIMEOUT)
        return Result(completed.returncode, completed.stdout, completed.stderr)

    def htalk(self, *words, code=0, db=True, env=None, cwd=None):
        """Run htalk, check its exit code and return its one JSON object."""
        result = self.run_raw(*words, db=db, env=env, cwd=cwd)
        detail = "argv=%r\nstdout=%s\nstderr=%s" % (words, result.stdout, result.stderr)
        self.assertEqual(code, result.code, detail)
        self.assertIsInstance(result.json, dict, detail)
        return result.json

    def error(self, *words, error=None, **kwargs):
        result = self.htalk(*words, code=2, **kwargs)
        self.assertEqual("error", result["state"])
        if error is not None:
            self.assertEqual(error, result["error"])
        return result

    def spawn(self, *words, db=True, env=None, argv=None):
        process = subprocess.Popen(argv or self.argv(words, db), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   text=True, encoding="utf-8", env=self.environment(env), cwd=self.tmp,
                                   preexec_fn=lambda: signal.signal(signal.SIGINT, signal.SIG_DFL))
        self.processes.append(process)
        return process

    def finish(self, process, code=0):
        stdout, stderr = process.communicate(timeout=TIMEOUT)
        result = Result(process.returncode, stdout, stderr)
        self.assertEqual(code, result.code, "stdout=%s\nstderr=%s" % (stdout, stderr))
        self.assertIsInstance(result.json, dict, stdout + stderr)
        return result.json

    def recover(self, command, code=0, env=None):
        """Execute a copyable recovery command with the htalk under test."""
        words = shlex.split(command)
        self.assertEqual("htalk", words[0], command)
        return self.htalk(*words[1:], code=code, db=False, env=env)

    def stop_leftovers(self):
        for process in self.processes:
            if process.poll() is None:
                process.kill()
                process.communicate()
        # A gated fake may outlive an interrupted htalk. Kill only processes still running this test's fakes.
        for entry in self.calls():
            try:
                if str(self.bin).encode() in Path("/proc/%d/cmdline" % entry["pid"]).read_bytes():
                    os.kill(entry["pid"], signal.SIGKILL)
            except (OSError, ProcessLookupError):
                pass
        for server in self.sockets:
            server.close()

    # Fake clients

    def configure(self, **settings):
        self.config.update(settings)
        (self.state / "config.json").write_text(json.dumps(self.config))

    def calls(self, client=None, argv=None):
        path = self.state / "calls.jsonl"
        entries = [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []
        return [entry for entry in entries if (client is None or entry["client"] == client)
                and (argv is None or entry.get("argv", [])[:len(argv)] == argv)]

    def release(self, gate):
        (self.state / (gate + ".release")).touch()

    def started(self, gate, timeout=15):
        wait_for(lambda: (self.state / (gate + ".started")).exists(), timeout, gate + " to start")

    # Registration

    def add_peer(self, name, harness="claude", session=None, workspace=None, *options, code=0):
        session = session or ("ses_" + uuid.uuid4().hex if harness == "opencode" else str(uuid.uuid4()))
        return self.htalk("peer", "add", name, "--harness", harness, "--session", session,
                          "--workspace", str(workspace or self.work), *options, code=code)

    def claude_recipient(self, name, workspace=None):
        """A registered Claude peer that the fake `claude agents --json` reports as live."""
        peer = self.add_peer(name, "claude", workspace=workspace)
        pid = 424200 + len(self.claude_rows)
        listener = ClaudeSocket(self.tmp / ("c%d.sock" % len(self.claude_rows)))
        self.sockets.append(listener)
        row = {"sessionId": peer["session_id"], "cwd": peer["workspace"], "pid": pid}
        (self.home / (".claude/sessions/%d.json" % pid)).write_text(
            json.dumps({**row, "messagingSocketPath": listener.path}))
        self.claude_rows.append(row)
        self.configure(claude_agents={**self.config.get("claude_agents", {}), "rows": self.claude_rows})
        return peer, listener

    def codex_recipient(self, name, archived=0, source="cli", saved=True):
        """A registered native Codex CLI peer with saved metadata in CODEX_HOME."""
        peer = self.add_peer(name, "codex")
        with closing(sqlite3.connect(self.codex_home / "state_5.sqlite")) as db, db:
            db.execute("CREATE TABLE IF NOT EXISTS threads (id TEXT PRIMARY KEY, cwd TEXT, archived INTEGER, source TEXT)")
            if saved:
                db.execute("INSERT INTO threads VALUES (?, ?, ?, ?)", (peer["session_id"], peer["workspace"], archived, source))
        return peer

    def native_claude(self, session_id, workspace=None, **metadata):
        """Environment for an htalk command run directly by this test process acting as Claude Code."""
        pid = os.getpid()
        saved = {"pid": pid, "sessionId": session_id, "procStart": process_start(pid),
                 "cwd": str(workspace or self.work), "messagingSocketPath": str(self.tmp / "unused.sock"), **metadata}
        (self.home / (".claude/sessions/%d.json" % pid)).write_text(json.dumps(saved))
        return {"CLAUDE_CODE_SESSION_ID": session_id, "CLAUDE_PID": str(pid)}

    def opencode_server(self, sessions):
        server = ThreadingHTTPServer(("127.0.0.1", 0), FakeOpenCode)
        server.state = {"requests": [], "sessions": sessions}
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        return server, "http://127.0.0.1:%d" % server.server_address[1]

    # Database inspection

    def sql(self, query, params=(), path=None):
        with closing(sqlite3.connect(path or self.db, timeout=5)) as db, db:
            return db.execute(query, params).fetchall()

    def columns(self, table, path=None):
        return [row[1] for row in self.sql("PRAGMA table_info(%s)" % table, path=path)]

    def user_version(self, path=None):
        return self.sql("PRAGMA user_version", path=path)[0][0]

    def waits(self):
        return self.sql("SELECT message_id, actor FROM waits")

    def insert_requests(self, sender, recipient, count, body="Question %d"):
        """Rows as an htalk 0.3 client writes them, in one transaction."""
        rows = [(str(uuid.uuid4()), sender, recipient, body % n, time.time()) for n in range(count)]
        with closing(sqlite3.connect(self.db, timeout=5)) as db, db:
            db.executemany("""INSERT INTO messages (id, sender, recipient, body, created_at, submission)
                VALUES (?, ?, ?, ?, ?, 'not_submitted')""", rows)
        return [row[0] for row in rows]
