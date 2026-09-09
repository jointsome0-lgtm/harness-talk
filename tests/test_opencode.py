"""OpenCode registration, reachability, one-attempt delivery and read-only discovery.
A synthetic loopback server imitates the documented routes; no real OpenCode runs."""
from base64 import b64encode
from contextlib import closing
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import sqlite3
import tempfile
import threading
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

from harness_talk import adapters, opencode
from harness_talk.store import Store


class FakeOpenCode(BaseHTTPRequestHandler):
    """Routes from the OpenAPI document of opencode 1.18.30, with invented data."""
    def log_message(self, *args):
        pass

    def reply(self, status, data=None):
        body = b"" if data is None else json.dumps(data).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def handle_one(self, method):
        state = self.server.state
        url = urlsplit(self.path)
        query = {k: v[0] for k, v in parse_qs(url.query).items()}
        length = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(length) or b"null")
        state["requests"].append((method, url.path, query, body))
        expected = "Basic " + b64encode(("opencode:" + state.get("password", "")).encode()).decode()
        if state.get("password") and self.headers.get("Authorization") != expected:
            return self.reply(401, {"name": "UnauthorizedError", "data": {"message": "Unauthorized"}})
        if state.get("hang"):
            self.connection.close()
            return
        parts = url.path.split("/")
        if (method, url.path) == ("GET", "/global/health"):
            return self.reply(200, {"healthy": True, "version": "1.18.30"})
        if (method, url.path) == ("GET", "/session"):
            return self.reply(200, list(state["sessions"].values()))
        if (method, url.path) == ("GET", "/session/status"):
            return self.reply(200, state["status"])
        session = state["sessions"].get(parts[2]) if len(parts) > 2 else None
        if session is None:
            return self.reply(404, {"name": "NotFoundError", "data": {"message": "Session not found: " + parts[2]}})
        if method == "GET" and len(parts) == 3:
            return self.reply(200, session)
        if method == "POST" and parts[3:] == ["prompt_async"]:
            return self.reply(state.get("prompt_status", 204))
        self.reply(404, {"name": "NotFoundError", "data": {"message": "no route"}})

    def do_GET(self):
        self.handle_one("GET")

    def do_POST(self):
        self.handle_one("POST")


def session(session_id, directory, **extra):
    return {"id": session_id, "slug": "synthetic", "projectID": "prj_synthetic", "directory": directory,
            "title": "synthetic", "version": "1.18.30", "time": {"created": 1000000, "updated": 1788990000000}, **extra}


class OpenCodeTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name).resolve()
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), FakeOpenCode)
        self.server.state = {"requests": [], "status": {}, "sessions": {"ses_synthetic": session("ses_synthetic", str(self.path))}}
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.addCleanup(self.server.shutdown)
        self.url = "http://127.0.0.1:%d" % self.server.server_address[1]
        self.store = Store(self.path / "mail.sqlite3")
        self.store.add_peer("alice", "claude", "b6ab4f5e-4c1a-4a35-9b3c-2c0b3f0e6f10", str(self.path))
        self.peer = self.store.add_peer("muse", "opencode", "ses_synthetic", str(self.path), url=self.url)

    def posts(self):
        return [r for r in self.server.state["requests"] if r[0] == "POST"]

    def test_registration_accepts_opaque_ids_and_migrates_existing_databases(self):
        self.assertEqual((self.peer["url"], self.peer["socket"], self.peer["session_id"]), (self.url, None, "ses_synthetic"))
        self.assertEqual(self.store.add_peer("default", "opencode", "ses_other", str(self.path))["url"], opencode.DEFAULT_URL)
        for kwargs, code in (({"session_id": "not-a-session"}, "invalid_opencode_session_id"),
                             ({"url": "http://example.invalid:4096"}, "opencode_url_must_be_loopback"),
                             ({"url": "http://user:secret@127.0.0.1:4096"}, "invalid_opencode_url"),
                             ({"socket": str(self.path / "x.sock")}, "opencode_uses_a_server_url_not_a_socket")):
            with self.assertRaisesRegex(ValueError, code):
                self.store.add_peer("bad", "opencode", kwargs.pop("session_id", "ses_bad"), str(self.path), **kwargs)
        with self.assertRaisesRegex(ValueError, "url_is_only_for_opencode"):
            self.store.add_peer("bad", "codex", "b6ab4f5e-4c1a-4a35-9b3c-2c0b3f0e6f11", str(self.path), url=self.url)
        with self.assertRaisesRegex(ValueError, "different_address"):
            self.store.add_peer("muse", "opencode", "ses_synthetic", str(self.path), url="http://localhost:4096")
        legacy = self.path / "legacy.sqlite3"
        with closing(sqlite3.connect(legacy)) as db, db:
            db.executescript("""CREATE TABLE peers (name TEXT PRIMARY KEY, harness TEXT NOT NULL, session_id TEXT NOT NULL,
                workspace TEXT NOT NULL, socket TEXT, UNIQUE(harness, session_id)); PRAGMA user_version=1;""")
            db.execute("INSERT INTO peers VALUES ('old', 'claude', 'b6ab4f5e-4c1a-4a35-9b3c-2c0b3f0e6f12', ?, NULL)", (str(self.path),))
        old = Store(legacy).peer("old")
        self.assertIsNone(old["url"])
        self.assertEqual(old, Store(legacy).add_peer(**{k: old[k] for k in old}))

    def test_probe_checks_exact_session_workspace_status_and_auth(self):
        self.assertEqual((adapters.probe(self.peer)["runtime_status"], adapters.probe(self.peer)["server_version"]), ("idle", "1.18.30"))
        self.server.state["status"] = {"ses_synthetic": {"type": "busy"}}
        self.assertEqual(opencode.probe(self.peer)["runtime_status"], "busy")
        self.assertTrue(all(q.get("directory") == str(self.path) for _, path, q, _ in self.server.state["requests"] if path.startswith("/session/")))
        sessions = self.server.state["sessions"]
        sessions["ses_synthetic"]["directory"] = str(self.path / "elsewhere")
        with self.assertRaisesRegex(ValueError, "recipient_identity_changed"):
            opencode.probe(self.peer)
        sessions["ses_synthetic"] = session("ses_synthetic", str(self.path), time={"created": 1, "updated": 2, "archived": 3})
        with self.assertRaisesRegex(ValueError, "recipient_session_archived"):
            opencode.probe(self.peer)
        del sessions["ses_synthetic"]
        with self.assertRaisesRegex(ValueError, "recipient_not_in_opencode_server"):
            opencode.probe(self.peer)
        sessions["ses_synthetic"] = session("ses_synthetic", str(self.path))
        self.server.state["password"] = "synthetic-secret"
        with patch.dict(os.environ, {}, clear=True):
            with self.assertRaisesRegex(ValueError, "opencode_unauthorized"):
                opencode.probe(self.peer)
        with patch.dict(os.environ, {"OPENCODE_SERVER_PASSWORD": "synthetic-secret"}):
            result = opencode.probe(self.peer)
        self.assertTrue(result["authenticated"])
        self.assertNotIn("synthetic-secret", json.dumps(result) + json.dumps(self.store.peer("muse")))
        with self.assertRaisesRegex(ValueError, "opencode_unreachable"):
            opencode.probe({**self.peer, "url": "http://127.0.0.1:1"})

    def test_single_attempt_outcomes_and_no_replay(self):
        question, _ = self.store.save("alice", "muse", "Synthetic question")
        result = self.store.notify_once(question["id"], adapters.notify)
        self.assertEqual((result["submission"], result["notification_detail"]), ("submitted", "opencode_prompt_async_accepted"))
        posts = self.posts()
        self.assertEqual(len(posts), 1)
        self.assertEqual(posts[0][1:3], ("/session/ses_synthetic/prompt_async", {"directory": str(self.path)}))
        self.assertIn(question["id"], posts[0][3]["parts"][0]["text"])
        self.assertEqual(posts[0][3]["parts"][0]["type"], "text")
        self.assertEqual(self.store.save("alice", "muse", "Synthetic question", question["id"])[1], False)
        self.assertEqual(self.store.notify_once(question["id"], adapters.notify)["submission"], "submitted")
        self.assertEqual(len(self.posts()), 1)
        outcomes = []
        for state, expected in (({"prompt_status": 404}, ("not_submitted", "recipient_not_in_opencode_server")),
                                ({"prompt_status": 500}, ("submission_unknown", "opencode_http_500"))):
            self.server.state.update(state)
            outcomes.append(opencode.notify(self.peer, "text"))
        self.assertEqual(outcomes, [("not_submitted", "recipient_not_in_opencode_server"), ("submission_unknown", "opencode_http_500")])
        del self.server.state["prompt_status"]
        before = len(self.posts())
        self.server.state["sessions"] = {}
        self.assertEqual(opencode.notify(self.peer, "text")[0], "not_submitted")
        self.assertEqual(len(self.posts()), before)  # A failed preflight never posts.
        self.assertEqual(opencode.notify({**self.peer, "url": "http://127.0.0.1:1"}, "text"), ("not_submitted", "opencode_unreachable"))
        self.server.state["sessions"] = {"ses_synthetic": session("ses_synthetic", str(self.path))}
        with patch.object(opencode, "probe", return_value=None):
            self.server.state["hang"] = True
            submission, detail = opencode.notify(self.peer, "text")
        self.assertEqual(submission, "submission_unknown")
        self.assertTrue(detail.startswith("opencode_"))
        self.assertNotIn("Traceback", detail)

    def test_discovery_is_read_only_and_separates_liveness(self):
        self.server.state["sessions"].update({
            "ses_child": session("ses_child", str(self.path), parentID="ses_synthetic"),
            "ses_gone": session("ses_gone", str(self.path), time={"created": 1, "updated": 2, "archived": 3}),
            "ses_busy": session("ses_busy", str(self.path / "other"))})
        self.server.state["status"] = {"ses_busy": {"type": "busy"}}
        saved = self.path / "opencode.db"
        with closing(sqlite3.connect(saved)) as db, db:
            db.execute("CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT NOT NULL, time_updated INTEGER NOT NULL, time_archived INTEGER)")
            db.executemany("INSERT INTO session VALUES (?,?,?,?,?)", [
                ("ses_synthetic", None, str(self.path), 1788990000000, None), ("ses_saved", None, "/synthetic/saved", 1788980000000, None),
                ("ses_archived", None, "/synthetic/x", 5, 6), ("ses_subagent", "ses_saved", "/synthetic/saved", 7, None)])
        result = opencode.discover([self.url, "http://127.0.0.1:1"], database=saved)
        found = {s["session_id"]: s for s in result["sessions"]}
        self.assertEqual(set(found), {"ses_synthetic", "ses_busy", "ses_saved"})
        self.assertEqual((found["ses_busy"]["runtime_status"], found["ses_synthetic"]["runtime_status"]), ("busy", "idle"))
        self.assertEqual((found["ses_saved"]["runtime_status"], found["ses_saved"]["runtime_reason"], found["ses_saved"]["url"]),
                         ("unknown", "saved_metadata_only", None))
        self.assertEqual(found["ses_synthetic"]["source"], "opencode_server")
        self.assertEqual([s["status"] for s in result["sources"]], ["ok", "unreachable", "ok"])
        self.assertEqual(self.posts(), [])
        self.assertNotIn("title", json.dumps(result))
        self.server.state["password"] = "synthetic-secret"
        with patch.dict(os.environ, {}, clear=True):
            result = opencode.discover([self.url], database=self.path / "absent.db")
        self.assertEqual([(s["status"], s.get("error")) for s in result["sources"]], [("unauthorized", "opencode_unauthorized"), ("missing", None)])
        self.assertEqual(result["sessions"], [])
        self.assertFalse((self.path / "absent.db").exists())


if __name__ == "__main__":
    unittest.main()
