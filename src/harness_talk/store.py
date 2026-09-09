"""One shared database. Reading never acknowledges or sends anything."""
from contextlib import closing
import math
import os
from pathlib import Path
import re
import sqlite3
import time
import uuid

from .opencode import valid_session_id as valid_opencode_session_id, valid_url as valid_opencode_url


def default_db():
    if os.environ.get("HTALK_DB"):
        return Path(os.environ["HTALK_DB"])
    return Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local/share")) / "harness-talk/mail.sqlite3"


def valid_text(body):
    if not isinstance(body, str) or not body.strip() or len(body.encode()) > 32000:
        raise ValueError("message_must_be_1_to_32000_bytes")


def valid_wait(seconds):
    if not math.isfinite(seconds) or not 0 <= seconds <= 45:
        raise ValueError("wait_seconds_must_be_between_0_and_45")


class Store:
    def __init__(self, path):
        self.path = Path(path).expanduser().resolve()
        self.path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        # Create privately before sqlite opens it, independent of the caller's umask.
        fd = os.open(self.path, os.O_CREAT | os.O_RDWR, 0o600)
        os.close(fd)
        with closing(self.connect()) as db, db:
            version = db.execute("PRAGMA user_version").fetchone()[0]
            if version not in (0, 1):
                raise ValueError("unsupported_database_version")
            db.executescript("""
                CREATE TABLE IF NOT EXISTS peers (
                    name TEXT PRIMARY KEY, harness TEXT NOT NULL,
                    session_id TEXT NOT NULL, workspace TEXT NOT NULL,
                    socket TEXT, UNIQUE(harness, session_id)
                );
                CREATE TABLE IF NOT EXISTS messages (
                    seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT UNIQUE NOT NULL,
                    sender TEXT NOT NULL REFERENCES peers(name),
                    recipient TEXT NOT NULL REFERENCES peers(name),
                    in_reply_to TEXT UNIQUE REFERENCES messages(id),
                    body TEXT NOT NULL, created_at REAL NOT NULL, ack_at REAL,
                    submission TEXT NOT NULL CHECK(submission IN
                        ('not_submitted', 'submission_unknown', 'submitted')),
                    notification_started_at REAL, notification_finished_at REAL,
                    notification_detail TEXT
                );
                PRAGMA user_version=1;
            """)
            # Additive: 0.2.0 databases gain a nullable server URL column for OpenCode peers.
            if "url" not in {row[1] for row in db.execute("PRAGMA table_info(peers)")}:
                db.execute("ALTER TABLE peers ADD COLUMN url TEXT")

    def connect(self):
        db = sqlite3.connect(self.path, timeout=5)
        db.row_factory = sqlite3.Row
        db.execute("PRAGMA foreign_keys=ON")
        return db

    def add_peer(self, name, harness, session_id, workspace, socket=None, url=None):
        if not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,63}", name):
            raise ValueError("invalid_peer_name")
        if harness not in ("codex", "claude", "opencode"):
            raise ValueError("unsupported_harness")
        # OpenCode identifiers are opaque; the other harnesses use UUIDs.
        session_id = valid_opencode_session_id(session_id) if harness == "opencode" else str(uuid.UUID(session_id))
        if harness == "opencode":
            if socket is not None:
                raise ValueError("opencode_uses_a_server_url_not_a_socket")
            url = valid_opencode_url(url)
        elif url is not None:
            raise ValueError("url_is_only_for_opencode")
        workspace = str(Path(workspace).expanduser().resolve(strict=True))
        if not Path(workspace).is_dir():
            raise ValueError("workspace_must_be_a_directory")
        if socket is not None:
            socket = str(Path(socket).expanduser().resolve())
        if harness == "claude" and socket is not None:
            raise ValueError("claude_socket_is_discovered_from_live_identity")
        values = (name, harness, session_id, workspace, socket, url)
        with closing(self.connect()) as db, db:
            db.execute("BEGIN IMMEDIATE")
            existing = db.execute("SELECT * FROM peers WHERE name=?", (name,)).fetchone()
            if existing:
                if tuple(existing) != values:
                    raise ValueError("peer_already_has_a_different_address")
            else:
                if db.execute("SELECT 1 FROM peers WHERE harness=? AND session_id=?",
                              (harness, session_id)).fetchone():
                    raise ValueError("session_already_has_a_peer_name")
                db.execute("INSERT INTO peers VALUES (?, ?, ?, ?, ?, ?)", values)
        return self.peer(name)

    def peer(self, name):
        with closing(self.connect()) as db:
            row = db.execute("SELECT * FROM peers WHERE name=?", (name,)).fetchone()
        if row is None:
            raise ValueError("unknown_peer")
        return dict(row)

    def peers(self):
        with closing(self.connect()) as db:
            return [dict(row) for row in db.execute("SELECT * FROM peers ORDER BY name")]

    def save(self, sender, recipient, body, message_id=None, in_reply_to=None):
        valid_text(body)
        self.peer(sender)
        self.peer(recipient)
        if sender == recipient:
            raise ValueError("sender_and_recipient_must_differ")
        message_id = str(uuid.UUID(message_id)) if message_id else str(uuid.uuid4())
        with closing(self.connect()) as db, db:
            db.execute("BEGIN IMMEDIATE")
            if in_reply_to:
                parent = db.execute("SELECT * FROM messages WHERE id=?", (in_reply_to,)).fetchone()
                if parent is None:
                    raise ValueError("unknown_request")
                if parent["in_reply_to"] is not None:
                    raise ValueError("reply_requires_a_request")
                if (sender, recipient) != (parent["recipient"], parent["sender"]):
                    raise ValueError("reply_address_mismatch")
                existing = db.execute("SELECT * FROM messages WHERE in_reply_to=?", (in_reply_to,)).fetchone()
                if existing:
                    if existing["body"] != body:
                        raise ValueError("reply_conflict_existing_answer_preserved")
                    return self.get(existing["id"]), False
            existing = db.execute("SELECT * FROM messages WHERE id=?", (message_id,)).fetchone()
            if existing:
                if tuple(existing[k] for k in ("sender", "recipient", "body", "in_reply_to")) != (sender, recipient, body, in_reply_to):
                    raise ValueError("message_id_conflict")
                return self.get(message_id), False
            db.execute("""INSERT INTO messages
                (id, sender, recipient, in_reply_to, body, created_at, submission)
                VALUES (?, ?, ?, ?, ?, ?, 'not_submitted')""",
                       (message_id, sender, recipient, in_reply_to, body, time.time()))
        return self.get(message_id), True

    def notify_once(self, message_id, notify):
        # Claim durably BEFORE crossing the client boundary. A crash stays unknown.
        with closing(self.connect()) as db, db:
            claimed = db.execute("""UPDATE messages SET submission='submission_unknown',
                notification_started_at=? WHERE id=? AND notification_started_at IS NULL""",
                                 (time.time(), message_id)).rowcount
        if not claimed:
            return self.get(message_id)
        message = self.get(message_id)
        try:
            state, detail = notify(self.peer(message["recipient"]), message, self.path)
            if state not in ("submitted", "not_submitted", "submission_unknown"):
                raise ValueError("invalid_notification_result")
        except Exception as exc:
            state, detail = "submission_unknown", type(exc).__name__
        with closing(self.connect()) as db, db:
            db.execute("""UPDATE messages SET submission=?, notification_detail=?,
                notification_finished_at=? WHERE id=?""", (state, detail, time.time(), message_id))
        return self.get(message_id)

    def get(self, message_id, actor=None):
        with closing(self.connect()) as db:
            row = db.execute("SELECT * FROM messages WHERE id=?", (message_id,)).fetchone()
            if row is None:
                raise ValueError("unknown_message")
            result = dict(row)
            if actor is not None and actor not in (result["sender"], result["recipient"]):
                raise ValueError("message_not_addressed_to_peer")
            answer = db.execute("SELECT * FROM messages WHERE in_reply_to=?", (message_id,)).fetchone()
        result["reply"] = dict(answer) if answer else None
        result["state"] = "reply_received" if answer else "saved"
        return result

    def inbox(self, actor):
        self.peer(actor)
        with closing(self.connect()) as db:
            rows = db.execute("""SELECT id FROM messages m WHERE recipient=? AND
                ((in_reply_to IS NULL AND NOT EXISTS
                  (SELECT 1 FROM messages r WHERE r.in_reply_to=m.id)) OR
                 (in_reply_to IS NOT NULL AND ack_at IS NULL)) ORDER BY seq""", (actor,)).fetchall()
        return {"messages": [self.get(row["id"]) for row in rows],
                "next_action": "Read and ack messages explicitly. Unanswered questions remain until replied to."}

    def sent(self, actor):
        self.peer(actor)
        with closing(self.connect()) as db:
            rows = db.execute("SELECT id FROM messages WHERE sender=? ORDER BY seq", (actor,)).fetchall()
        return {"messages": [self.get(row["id"]) for row in rows]}

    def ack(self, message_id, actor):
        with closing(self.connect()) as db, db:
            if not db.execute("""UPDATE messages SET ack_at=COALESCE(ack_at, ?)
                WHERE id=? AND recipient=?""", (time.time(), message_id, actor)).rowcount:
                raise ValueError("only_recipient_can_ack")
        return self.get(message_id, actor)

    def wait(self, message_id, actor, seconds):
        valid_wait(seconds)
        request = self.get(message_id, actor)
        if request["sender"] != actor or request["in_reply_to"] is not None:
            raise ValueError("wait_requires_own_request")
        deadline = time.monotonic() + seconds
        while True:
            result = self.get(message_id, actor)
            if result["reply"]:
                return result
            if time.monotonic() >= deadline:
                result["wait_ended"] = "timeout"
                return result
            time.sleep(min(.1, max(0, deadline - time.monotonic())))
