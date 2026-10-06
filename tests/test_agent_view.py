"""The agent view: every text and JSON shape an agent reads from htalk.

The files under tests/agent_view/ describe that view in plain text. Each test
captures one part again, through the executable or MCP stdio, and compares it
with its file. Nothing here imports the implementation; the list of fixed codes
is read from the source text, because no command prints it.

After an intended change, regenerate the files and review their diff:

    python3 -B tests/test_agent_view.py --update

Print the size report, freshly measured:

    python3 -B tests/test_agent_view.py --size
"""
from contextlib import closing
import json
import os
from pathlib import Path
import re
import shlex
import signal
import sqlite3
import sys
import textwrap
import unittest
import uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from compat_support import REPO, HtalkCase, wait_for  # noqa: E402
from test_mcp import McpClient  # noqa: E402

SNAPSHOT = Path(__file__).resolve().parent / "agent_view"
UPDATE = SIZE = False
REGENERATE = "python3 -B tests/test_agent_view.py --update"
UUID = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
# Row and address fields echo the fixture or the clock. Their keys are listed; their values are not.
ECHOED = {"id", "seq", "sender", "recipient", "in_reply_to", "body", "created_at", "ack_at", "notification_started_at",
          "notification_finished_at", "wait_returned_at", "name", "session_id", "workspace", "socket", "url", "retired_at"}
# What htalk declares in the tool's input schema; the rest comes from the MCP library.
SCHEMA_KEYS = {"type", "properties", "required", "additionalProperties", "items", "description", "enum", "default"}

LITERAL = r'"((?:[^"\\]|\\.)*)"'
ERROR_CODE = re.compile(r"\b(?:Error::code|(?<![:.\w])code)\(\s*" + LITERAL)
DETAIL_CODE = re.compile(r"(?:\b(?:Failure::coded|Failure::Coded|Outcome::(?:submitted|not_submitted|unknown))"
                         r"|(?<![:.\w])coded|\.detail)\(\s*" + LITERAL)
RUNTIME_CODE = re.compile(r'format!\(\s*"([a-z]+_[a-z_]*:?\{[^"]*)"')
CLASS_NAME = re.compile(r'"([A-Z][A-Za-z]*(?:Error|Exception|Interrupt|Expired)|BadStatusLine|IncompleteRead|InvalidURL'
                        r'|LineTooLong|RemoteDisconnected|UnknownProtocol)"')


def key_tree(value):
    """The key set of a JSON value: "a, b{c, d}, e[{f}]"."""
    if isinstance(value, dict):
        return "{%s}" % ", ".join(key + key_tree(value[key]) for key in sorted(value))
    if isinstance(value, list):
        return "[%s]" % " | ".join(sorted({key_tree(item) for item in value} - {""}))
    return ""


def leaves(value, path=""):
    if isinstance(value, dict):
        for key in sorted(value):
            yield from leaves(value[key], "%s.%s" % (path, key) if path else key)
    elif isinstance(value, list):
        for index, item in enumerate(value):
            yield from leaves(item, "%s[%d]" % (path, index))
    else:
        yield path, value


def tokens(text):
    """The parent issue's measure: UTF-8 bytes / 3.3."""
    return round(len(text.encode()) / 3.3)


class AgentView(HtalkCase):
    def setUp(self):
        super().setUp()
        self.placeholders = [(str(self.db), "DATABASE"), (str(self.work), "WORKSPACE"), (str(self.home), "HOME"),
                             (str(self.codex_home), "CODEX_HOME"), (str(self.tmp), "TMP")]
        self.lines = []

    # Snapshot files

    def check(self, name, text):
        path = SNAPSHOT / name
        text = "\n".join(line.rstrip() for line in text.splitlines()) + "\n"
        if UPDATE:
            path.parent.mkdir(exist_ok=True)
            path.write_text(text, encoding="utf-8")
            return
        self.assertTrue(path.is_file(), "missing %s; create it with: %s" % (path, REGENERATE))
        self.assertEqual(path.read_text(encoding="utf-8"), text,
                         "The agent view changed. If that is intended, run: %s" % REGENERATE)

    # Fixture values become stable names

    def named(self, value, label):
        self.placeholders.append((str(value), label))
        return value

    def plain(self, text):
        for value, label in sorted(self.placeholders, key=lambda pair: -len(pair[0])):
            text = text.replace(value, label)
        return UUID.sub("UUID", text)

    def entry(self, title, words, db, code):
        self.lines += ["", "## " + title, "$ " + shlex.join(["htalk", *self.argv(words, db)[len(self.command):]]),
                       "exit %d" % code]

    def record(self, title, words, code, result, db=True):
        """One result: its command, exit code, key set, and every value that is not an echo."""
        self.entry(title, words, db, code)
        self.lines.append("keys: " + key_tree(result)[1:-1])
        for path, value in leaves(result):
            if re.split(r"[.\[]", path)[0] in ("recovery", "next_action") or path.rpartition(".")[2] not in ECHOED:
                self.lines.append("%s = %s" % (path, json.dumps(value, ensure_ascii=False)))

    def step(self, title, *words, code=0, env=None, db=True):
        result = self.htalk(*words, code=code, env=env, db=db)
        self.record(title, words, code, result, db)
        return result

    def said(self, title, *words, db=False):
        """A command that answers on stderr alone."""
        result = self.run_raw(*words, db=db)
        self.assertEqual((2, ""), (result.code, result.stdout), words)
        self.entry(title, words, db, 2)
        self.lines += ["stderr:", *("  " + line for line in result.stderr.splitlines())]

    def interrupt(self, title, words, ready):
        """Ctrl-C once the command reached the named point."""
        process = self.spawn(*words)
        ready()
        process.send_signal(signal.SIGINT)
        result = self.finish(process, code=130)
        self.record(title, words, 130, result)
        return result

    # Captures

    def help_pages(self):
        """The rendered --help of every command, found by walking each Commands list."""
        pages = {}

        def walk(path):
            result = self.run_raw(*path, "--help", db=False, env={"COLUMNS": "100"})
            self.assertEqual((0, ""), (result.code, result.stderr), path)
            pages[" ".join(["htalk", *path])] = result.stdout
            listed = re.search(r"^Commands:\n((?:  \S.*\n)+)", result.stdout, re.M)
            for name in re.findall(r"^  (\S+)", listed.group(1), re.M) if listed else ():
                walk([*path, name])

        walk([])
        return pages

    def mcp_view(self):
        """What htalk supplies over MCP stdio: instructions, the one tool, and its scope refusals."""
        def declared(schema):
            if isinstance(schema, dict):
                return {key: declared(value) if key != "properties" else {name: declared(item) for name, item in value.items()}
                        for key, value in schema.items() if key in SCHEMA_KEYS}
            return schema

        for peer in ("alice", "bob"):
            self.htalk("peer", "add", peer, "--harness", "generic", "--delivery", "pull")
        local = McpClient(self)
        tool, = local.request("tools/list")["result"]["tools"]
        view = {"server": local.info["result"]["serverInfo"]["name"],
                "instructions": local.info["result"]["instructions"],
                "tool": tool["name"], "description": tool["description"], "schema": declared(tool["inputSchema"])}
        refusals = []
        for args in (["watch"], ["peer", "retire", "bob"], ["--as", "bob", "inbox"],
                     ["send", "bob", "--id", str(uuid.uuid4()), "--message-file", "/dev/null"],
                     ["send", "bob", "--message", "No retry identity"]):
            result = local.call(*args)
            self.assertTrue(result["isError"], args)
            refusals.append((args, result["content"][0]["text"]))
        view["refusals"] = refusals
        view["result"] = key_tree(local.call("inbox"))
        local.close()
        remote = McpClient(self, connector=self.argv(["--as", "alice", "mcp"], True))
        view["remote_instructions"] = remote.info["result"]["instructions"]
        remote.close()
        return view

    def empty_trust(self):
        """A private list of known devices that names none."""
        path = self.tmp / "trust.json"
        path.write_text(json.dumps({"schema_version": 1, "devices": []}))
        path.chmod(0o600)
        return self.named(str(path), "TRUST")

    def notification(self):
        """The watch events, and the notice text watch emits and each fake native client is handed."""
        def send(recipient):
            request = self.named(str(uuid.uuid4()), "MESSAGE_ID")
            self.htalk("--as", "alice", "send", recipient, "--id", request, "--message", "Invented question")

        for peer in ("alice", "bob"):
            self.htalk("peer", "add", peer, "--harness", "generic", "--delivery", "pull")
        send("bob")
        watcher = self.spawn("--as", "bob", "watch")
        events = [json.loads(watcher.stdout.readline()) for _ in range(2)]
        watcher.send_signal(signal.SIGINT)
        watcher.communicate(timeout=15)
        self.assertEqual(130, watcher.returncode)
        _, listener = self.claude_recipient("carol")
        self.codex_recipient("dave")
        self.configure(codex_queue={"mode": "ok", "queue_id": str(uuid.uuid4())})
        server, url = self.opencode_server({"ses_muse": {"id": "ses_muse", "directory": str(self.work),
                                                         "time": {"created": 1, "updated": 2}}})
        self.add_peer("muse", "opencode", "ses_muse", None, "--url", url)
        for peer in ("carol", "dave", "muse"):
            send(peer)
        queued = self.calls("codex", ["queue"])[-1]["argv"]
        posted = [request for request in server.state["requests"] if request["method"] == "POST"]
        notices = {"htalk watch": events[1]["notification"],
                   "Claude Code": listener.frames()[-1]["message"]["content"],
                   "the Codex CLI queue": queued[queued.index("--message") + 1],
                   "OpenCode": posted[-1]["body"]["parts"][0]["text"]}
        return events, {client: re.sub(r"--as \w+ ", "--as RECIPIENT ", self.plain(text)) for client, text in notices.items()}

    # Tests

    def test_help(self):
        pages = self.help_pages()
        text = ["# The rendered --help of every htalk command and subcommand, as the executable prints it.", ""]
        for command, page in pages.items():
            text += ["==> %s --help" % command, page.rstrip("\n"), ""]
        self.check("help.txt", "\n".join(text))

    def test_mcp(self):
        view = self.mcp_view()
        text = ["# What htalk supplies over MCP stdio. Everything else in the handshake comes from the MCP library.",
                "", "server name: " + view["server"], "",
                "instructions, htalk --as alice mcp:", view["instructions"], "",
                "instructions, htalk mcp --connect -- COMMAND:", view["remote_instructions"], "",
                "tool: " + view["tool"], "", "description:", view["description"], "",
                "input schema:", json.dumps(view["schema"], indent=2, sort_keys=True), "",
                "result of a call that ran: " + view["result"], "",
                "refusals, returned as an error text before any command runs:"]
        for args, message in view["refusals"]:
            text += [json.dumps(args), "  " + message]
        self.check("mcp.txt", self.plain("\n".join(text)))

    def test_notification(self):
        (ready, message), notices = self.notification()
        text = ["# The notice a recipient's client receives. It never contains the message body.", "",
                "watch events, one JSON object per line:", "  " + key_tree(ready)[1:-1] + "    event = " + ready["event"],
                "  " + key_tree(message)[1:-1] + "    event = " + message["event"]]
        for client, notice in notices.items():
            same = [earlier for earlier, other in notices.items() if other == notice][0]
            text += ["", "notification, %s:" % client, notice if same == client else "the same text as %s" % same]
        self.check("notification.txt", "\n".join(text))

    def test_results(self):
        self.lines = ["# Results of commands that worked, that saved a message with an unconfirmed notice,",
                      "# or that were interrupted. Each entry: the command, its exit code, the key set, and",
                      "# every value that is not an echo of the fixture. Echoed row and address fields:",
                      *textwrap.wrap(", ".join(sorted(ECHOED)) + ".", 96, initial_indent="# ", subsequent_indent="# ")]
        self.result_scenarios()
        self.check("results.txt", self.plain("\n".join(self.lines)))

    def test_errors(self):
        self.lines = ["# Fixed codes, read from the source text, and what a failed command returns.",
                      "#",
                      "# The lists hold every string literal the source passes to Error::code, to",
                      "# Failure::coded, to a notification outcome or to a cleanup detail. Values that the",
                      "# source spells another way, such as a skipped notice's reason, appear in the entries",
                      "# below and in results.txt, not in the lists.",
                      "#",
                      "# A code appears as error in a command's error result, or as a detail:",
                      "# notification_detail, notification_cleanup.detail, a discovery source's detail.",
                      "# A failed check returns its detail code as error. Where no fixed code applies, a",
                      "# detail is a documented exception class name."]
        scanned = self.scan_codes()
        for title, key in (("error codes", "error"), ("detail codes", "detail"),
                           ("codes built at run time, {} is the variable part", "runtime"),
                           ("exception class names", "class")):
            self.lines += ["", "## " + title, *scanned[key]]
        self.lines += ["", "# What each failure returns. A code without its own entry gets one of the two",
                       "# default texts: 'error, no peer selected' or 'error, peer selected'."]
        self.error_scenarios()
        self.check("errors.txt", self.plain("\n".join(self.lines)))

    def test_size(self):
        pages = self.help_pages()
        view = self.mcp_view()
        self.db = self.tmp / "notice" / "mail.sqlite3"
        self.placeholders.insert(0, (str(self.db), "DATABASE"))
        notice = self.notification()[1]["htalk watch"]
        schema = json.dumps(view["schema"], separators=(",", ":"))
        groups = [command for command, page in pages.items() if re.search(r"^Commands:$", page, re.M)]
        rows = [("commands", len(pages) - 1, ""),
                ("  of which groups with subcommands", len(groups) - 1, ""),
                ("help, all pages", sum(map(len, pages.values())), tokens("".join(pages.values()))),
                ("help, htalk --help alone", len(pages["htalk"]), tokens(pages["htalk"])),
                ("MCP tool description", len(view["description"]), tokens(view["description"])),
                ("MCP input schema", len(schema), tokens(schema)),
                ("MCP server instructions", len(view["instructions"]), tokens(view["instructions"])),
                ("notification text", len(notice), tokens(notice))]
        text = ["# How much text an agent loads. Characters as printed; tokens are UTF-8 bytes / 3.3.",
                "# The notification is measured as notification.txt writes it, with names for its three values.", "",
                "%-38s %10s %8s" % ("", "characters", "tokens")]
        text += ["%-38s %10s %8s" % row for row in rows]
        text += ["", "help page by page:"]
        text += ["%-38s %10d %8d" % (command + " --help", len(page), tokens(page)) for command, page in pages.items()]
        if SIZE:
            print("\n".join(text))
        self.check("size.txt", "\n".join(text))

    # Fixed codes in the source

    def scan_codes(self):
        source = REPO / "src"
        self.assertTrue(source.is_dir(), "the list of fixed codes is read from %s; run this test in a checkout" % source)
        text = "\n".join(path.read_text(encoding="utf-8") for path in sorted(source.rglob("*.rs")))
        found = {"error": set(ERROR_CODE.findall(text)), "detail": set(DETAIL_CODE.findall(text)),
                 "class": set(CLASS_NAME.findall(text)),
                 "runtime": {re.sub(r"\{[^}]*\}", "{}", code) for code in RUNTIME_CODE.findall(text)}}
        found["detail"] -= found["class"]
        return {key: sorted(values) for key, values in found.items()}

    # Scenarios

    def result_scenarios(self):
        step = self.step
        work = str(self.work)
        carol_session = self.named(str(uuid.uuid4()), "CAROL_SESSION")
        carol = ["peer", "add", "carol", "--harness", "claude", "--session", carol_session, "--workspace", work]

        self.lines += ["", "# Peers"]
        step("a native peer", *carol)
        step("a pull peer", "peer", "add", "helper", "--harness", "generic", "--delivery", "pull")
        alice, listener = self.claude_recipient("alice")
        self.named(alice["session_id"], "ALICE_SESSION")
        self.named(listener.path, "ALICE_SOCKET")
        bob = self.codex_recipient("bob")
        self.named(bob["session_id"], "BOB_SESSION")
        queue_id = self.named(str(uuid.uuid4()), "QUEUE_ID")
        self.configure(codex_queue={"mode": "ok", "queue_id": queue_id})
        server, url = self.opencode_server({"ses_muse": {"id": "ses_muse", "directory": work,
                                                         "time": {"created": 1, "updated": 2}}})
        self.named(url, "OPENCODE_URL")
        self.add_peer("muse", "opencode", "ses_muse", None, "--url", url)
        step("check a live Claude session", "peer", "check", "alice")
        step("check a saved Codex CLI session", "peer", "check", "bob")
        step("check an OpenCode session", "peer", "check", "muse")
        step("check a pull peer", "peer", "check", "helper")
        step("retire a peer", "peer", "retire", "carol")
        step("list active peers", "peer", "list")
        step("register a retired peer again", *carol)
        step("restore a peer", "peer", "restore", "carol")
        self.htalk("peer", "retire", "muse")
        step("list every peer", "peer", "list", "--all")
        self.htalk("peer", "restore", "muse")
        step("discover sessions", "peer", "discover", "--workspace", work, "--opencode-url", url)
        step("prepare the mailbox", "migrate")

        self.lines += ["", "# A request and its answer"]
        request = self.named(str(uuid.uuid4()), "REQUEST_ID")
        send = ["--as", "helper", "send", "alice", "--id", request, "--message", "Invented question"]
        step("send, the notice was submitted", *send)
        step("the same send again", *send)
        step("the recipient's inbox", "--as", "alice", "inbox")
        step("the recipient shows the request", "--as", "alice", "show", request)
        step("the recipient acknowledges; Claude has no notice removal", "--as", "alice", "ack", request)
        reply = step("reply to a pull peer", "--as", "alice", "reply", request, "--message", "Invented answer")
        self.named(reply["id"], "REPLY_ID")
        step("wait returns the answer", "--as", "helper", "wait", request, "--seconds", "0")
        step("the sender's outgoing list", "--as", "helper", "sent")
        step("the sender acknowledges the answer", "--as", "helper", "ack", reply["id"])
        step("the request after its answer was read", "--as", "helper", "show", request)
        self.sql("UPDATE messages SET notification_detail='returned_by_recipient_wait' WHERE id=?", (reply["id"],))
        step("an answer the recipient's wait returned", "--as", "alice", "show", reply["id"])

        self.lines += ["", "# Notification outcomes and cleanup"]
        queued = self.named(str(uuid.uuid4()), "QUEUED_ID")
        step("send, queued by the Codex CLI", "--as", "helper", "send", "bob", "--id", queued, "--message", "Invented task")
        step("acknowledge, the queued notice is removed", "--as", "bob", "ack", queued)
        self.configure(codex_app_server={"deleted": False})
        step("acknowledge again, the notice is already gone", "--as", "bob", "ack", queued)
        quiet = self.named(str(uuid.uuid4()), "QUIET_ID")
        step("send without a notice", "--as", "helper", "send", "bob", "--id", quiet, "--message", "Poll for this", "--no-notify")
        step("acknowledge a message that had no notice", "--as", "bob", "ack", quiet)
        step("wait ends without an answer", "--as", "helper", "wait", quiet, "--seconds", "0")
        self.sql("UPDATE messages SET submission='submission_unknown', notification_started_at=1 WHERE id=?", (quiet,))
        step("acknowledge while the sender is still submitting", "--as", "bob", "ack", quiet)
        codex_state = self.codex_home / "state_5.sqlite"
        threads = self.sql("SELECT * FROM threads", path=codex_state)
        self.sql("DELETE FROM threads", path=codex_state)
        step("acknowledge when Codex state cannot be verified", "--as", "bob", "ack", queued)
        for row in threads:
            self.sql("INSERT INTO threads VALUES (?, ?, ?, ?)", row, path=codex_state)
        step("send to an OpenCode session", "--as", "helper", "send", "muse", "--message", "Invented prompt")
        step("send, the recipient's client is not running", "--as", "helper", "send", "carol", "--message", "Anyone there?", code=2)
        late = self.named(str(uuid.uuid4()), "LATE_ID")
        self.htalk("--as", "carol", "send", "helper", "--id", late, "--message", "Before retiring", "--no-notify")
        self.htalk("peer", "retire", "carol")
        step("reply to a retired peer", "--as", "helper", "reply", late, "--message", "Late answer")

        self.lines += ["", "# Pages"]
        step("an inbox page with more behind it", "--as", "bob", "inbox", "--limit", "1")
        step("an outgoing page with more behind it", "--as", "helper", "sent", "--limit", "1", "--bodies")

        self.lines += ["", "# Interruption"]
        self.interrupt("Ctrl-C during wait", ["--as", "helper", "wait", queued, "--seconds", "45"],
                       lambda: wait_for(self.waits, message="the wait to register"))
        stuck = self.named(str(uuid.uuid4()), "STUCK_ID")
        send = ["--as", "helper", "send", "bob", "--id", stuck, "--message", "Invented task"]
        self.configure(codex_queue={"mode": "block"})
        self.interrupt("Ctrl-C while the notice is being submitted", send, lambda: self.started("codex_queue"))
        self.configure(codex_queue={"mode": "ok", "queue_id": queue_id}, codex_app_server={"mode": "block"})
        step("the same send after the interruption", *send)
        noticed = self.named(str(uuid.uuid4()), "NOTICED_ID")
        self.htalk("--as", "helper", "send", "bob", "--id", noticed, "--message", "Invented task")
        self.interrupt("Ctrl-C while an acknowledged notice is being removed", ["--as", "bob", "ack", noticed],
                       lambda: self.started("codex_delete"))

        self.lines += ["", "# Catalogue"]
        config = self.named(str(self.tmp / "catalog.json"), "CATALOG")
        published = step("publish a profile", "--as", "helper", "catalog", "publish", "--config", config, "bob",
                         "--name", "Reviewer", "--role", "Check requested work", "--device-name", "Known device")
        self.named(published["profile_id"], "PROFILE_ID")
        step("the published catalogue", "catalog", "export", "--config", config, db=False)
        step("withdraw a profile", "catalog", "unpublish", "--config", config, published["profile_id"], db=False)
        step("discover with no known device", "catalog", "discover", "--trust", self.empty_trust(), "--via", "ssh", db=False)

    def error_scenarios(self):
        work = str(self.work)
        session = self.named(str(uuid.uuid4()), "SESSION_ID")
        other = self.named(str(uuid.uuid4()), "OTHER_SESSION")
        unknown = self.named(str(uuid.uuid4()), "UNKNOWN_ID")

        def fail(*words, env=None, title=None, db=True):
            result = self.htalk(*words, code=2, env=env, db=db)
            self.assertEqual("error", result["state"], words)
            self.record(title or result["error"], words, 2, result, db)
            return result

        self.lines += ["", "# Before a mailbox exists"]
        fail("peer", "list")
        fail("peer", "add", "alice", "--harness", "aider", "--session", session, "--workspace", work)
        fail("peer", "add", "alice", "--harness", "claude", "--session", "not-a-uuid", "--workspace", work,
             title="error, no peer selected")
        fail("peer", "discover", "--harness", "claude", "--codex-socket", "/nowhere/codex.sock")

        self.lines += ["", "# Registration"]
        alice = ["peer", "add", "alice", "--harness", "claude", "--session", session, "--workspace", work]
        self.htalk(*alice)
        self.htalk("peer", "add", "bob", "--harness", "codex", "--session", other, "--workspace", work)
        self.htalk("peer", "add", "eve", "--harness", "generic", "--delivery", "pull")
        fail("peer", "add", "alice", "--harness", "claude", "--session", other, "--workspace", work)
        fail("peer", "add", "carol", "--harness", "claude", "--session", session, "--workspace", work)
        fail("peer", "check", "alice", title="a failed check returns its detail code")

        self.lines += ["", "# Choosing the peer"]
        fail("inbox")
        current = self.named(str(uuid.uuid4()), "CURRENT_SESSION")
        native = self.native_claude(current)
        fail("inbox", env=native, title="peer_required_use_as_or_HTALK_PEER, in a recognized Claude Code session")
        fail("--as", "alice", "inbox", env=native)
        fail("--as", "bob", "inbox", env={"CODEX_THREAD_ID": current})
        fail("--as", "zed", "inbox", title="unknown_peer, the name given with --as")

        self.lines += ["", "# Messages"]
        fail("--as", "alice", "send", "zed", "--message", "Invented question", title="error, peer selected")
        request = self.named(str(uuid.uuid4()), "REQUEST_ID")
        send = ["--as", "alice", "send", "bob", "--id", request, "--message", "Invented question", "--no-notify"]
        self.htalk(*send)
        fail(*send[:-2], "A different question", "--no-notify")
        hidden = self.named(self.htalk("--as", "eve", "send", "bob", "--message", "Private", "--no-notify")["id"], "HIDDEN_ID")
        fail("--as", "alice", "send", "bob", "--id", hidden, "--message", "Invented question", "--no-notify",
             title="message_id_conflict, the ID belongs to a conversation this peer cannot read")
        self.htalk("--as", "bob", "reply", request, "--message", "Invented answer", "--no-notify")
        fail("--as", "bob", "reply", request, "--message", "A different answer", "--no-notify")
        fail("--as", "alice", "show", unknown)
        self.htalk("peer", "retire", "bob")
        fail("--as", "alice", "send", "bob", "--message", "After retirement")

        self.lines += ["", "# A retired registration"]
        fail("peer", "check", "bob", title="a failed check of a retired peer; its detail is a class name")
        fail("peer", "add", "bob", "--harness", "codex", "--session", session, "--workspace", work,
             title="peer_already_has_a_different_address, the registered peer is retired")
        fail("peer", "add", "carol", "--harness", "codex", "--session", other, "--workspace", work,
             title="session_already_has_a_peer_name, the registered peer is retired")

        self.lines += ["", "# Mailbox preparation"]
        mailbox = self.db
        self.db = self.tmp / "unversioned" / "mail.sqlite3"
        self.db.parent.mkdir()
        self.sql("CREATE TABLE notes (text TEXT)")
        self.placeholders.insert(0, (str(self.db), "DATABASE"))
        fail("migrate")
        fail("peer", "list", title="unsupported_unversioned_database, from an ordinary command")
        self.sql("DROP TABLE notes")
        self.sql("PRAGMA user_version=4")
        fail("migrate", title="migrate, any other failure")
        for code, damage in (("database_backup_failed", None), ("database_foreign_key_violation", "DELETE FROM peers"),
                             ("database_integrity_check_failed", "UPDATE messages SET submission='invalid'")):
            self.db = self.tmp / code / "mail.sqlite3"
            self.db.parent.mkdir()
            self.placeholders.insert(0, (str(self.db), "DATABASE"))
            self.legacy_mailbox(damage)
            if damage is None:
                Path(str(self.db) + ".backups").write_text("not a directory")
            self.assertEqual(code, fail("peer", "list")["error"])
        self.db = mailbox

        self.lines += ["", "# Catalogue. Its errors are a code, or the operating system's text for a file error."]
        config = self.named(str(self.tmp / "catalog.json"), "CATALOG")
        trust = self.empty_trust()
        publish = ["catalog", "publish", "--config", config, "eve", "--name", "Reviewer", "--role", "Check requested work"]
        fail(*publish, db=False)
        fail("catalog", "export", "--config", config, db=False, title="a file error")
        self.htalk("--as", "alice", *publish)
        fail("catalog", "unpublish", "--config", config, unknown, db=False)
        fail("catalog", "discover", "--trust", trust, "--interface", "htalk-missing0", "--seconds", "1", db=False)

        self.lines += ["", "# Answers on stderr. Nothing is printed on stdout."]
        self.said("a command line the parser refuses", "send")
        self.said("an unknown command", "inboxx")
        self.said("no command")
        self.said("mcp without a peer", "mcp", db=True)
        self.said("mcp --connect with a local mailbox", "mcp", "--connect", "--", "true", db=True)
        self.said("receive with a local mailbox", "receive", "--peer", "alice", "--session", session, "--workspace", work,
                  "--state", self.named(str(self.tmp / "receiver"), "RECEIVER_STATE"), "--", "true", db=True)
        self.said("catalog connect, no such profile", "catalog", "connect", "--trust", trust, "--via", "ssh", "Reviewer")

    def legacy_mailbox(self, damage):
        """A schema 1 mailbox with one message, optionally damaged the way a failed migration finds it."""
        with closing(sqlite3.connect(self.db)) as db, db:
            db.executescript("""
                CREATE TABLE peers (name TEXT PRIMARY KEY, harness TEXT NOT NULL, session_id TEXT NOT NULL,
                    workspace TEXT NOT NULL, socket TEXT, UNIQUE(harness, session_id));
                CREATE TABLE messages (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT UNIQUE NOT NULL,
                    sender TEXT NOT NULL REFERENCES peers(name), recipient TEXT NOT NULL REFERENCES peers(name),
                    in_reply_to TEXT UNIQUE REFERENCES messages(id), body TEXT NOT NULL, created_at REAL NOT NULL, ack_at REAL,
                    submission TEXT NOT NULL CHECK(submission IN ('not_submitted', 'submission_unknown', 'submitted')),
                    notification_started_at REAL, notification_finished_at REAL, notification_detail TEXT);
                PRAGMA user_version=1;""")
            for name in ("alice", "bob"):
                db.execute("INSERT INTO peers VALUES (?, 'claude', ?, ?, NULL)", (name, str(uuid.uuid4()), str(self.work)))
            db.execute("""INSERT INTO messages (id, sender, recipient, body, created_at, submission)
                VALUES (?, 'alice', 'bob', 'Invented question', 1.0, 'not_submitted')""", (str(uuid.uuid4()),))
            if damage:
                db.execute("PRAGMA ignore_check_constraints=ON")
                db.execute(damage)


if __name__ == "__main__":
    UPDATE, SIZE = "--update" in sys.argv, "--size" in sys.argv
    sys.argv = [word for word in sys.argv if word not in ("--update", "--size")]
    unittest.main(defaultTest="AgentView.test_size" if SIZE and len(sys.argv) == 1 else None)
