"""htalk: durable local messages with optional client notifications."""
import argparse
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys

from . import __version__
from .adapters import notify, probe
from .store import Store, default_db, valid_wait


def parser():
    root = argparse.ArgumentParser(description=__doc__)
    root.add_argument("--version", action="version", version=__version__)
    root.add_argument("--db", type=Path, default=default_db())
    root.add_argument("--as", dest="actor", default=os.environ.get("HTALK_PEER"), help="Your registered peer name; also HTALK_PEER.")
    commands = root.add_subparsers(dest="command", required=True)
    peer = commands.add_parser("peer").add_subparsers(dest="peer_command", required=True)
    add = peer.add_parser("add", help="Save an immutable concrete address. Does not notify or launch it.")
    add.add_argument("name")
    add.add_argument("--harness", choices=("codex", "claude"), required=True)
    add.add_argument("--session", required=True)
    add.add_argument("--workspace", required=True)
    add.add_argument("--socket", help="Optional Codex standalone app-server socket; default uses native codex queue.")
    peer.add_parser("list")
    check = peer.add_parser("check", help="Verify available identity evidence without messaging.")
    check.add_argument("name")
    send = commands.add_parser("send", help="Save a request once; optional active wait.")
    send.add_argument("recipient")
    send.add_argument("--id", help="Caller-generated UUID for safe retry after interrupted output.")
    reply = commands.add_parser("reply", help="Answer the exact request. Identical retries do not notify again.")
    reply.add_argument("message_id")
    for cmd in (send, reply):
        text = cmd.add_mutually_exclusive_group(required=True)
        text.add_argument("--message")
        text.add_argument("--message-file", type=Path)
        cmd.add_argument("--no-notify", action="store_true", help="Save for inbox retrieval only.")
    send.add_argument("--wait", type=float, default=0, help="Wait 0–45 seconds; no model calls or retries.")
    wait = commands.add_parser("wait", help="Wait again on a saved request, without another send.")
    wait.add_argument("message_id")
    wait.add_argument("--seconds", type=float, default=45)
    for name in ("show", "ack"):
        commands.add_parser(name).add_argument("message_id")
    commands.add_parser("inbox", help="Incoming unanswered questions and unacknowledged answers.")
    commands.add_parser("sent", help="Recover outgoing IDs after interruption, including uncertain notifications.")
    return root


def main(argv=None):
    args = parser().parse_args(argv)
    try:
        attempted_notification = False
        os.umask(0o077)
        if args.command == "send":
            valid_wait(args.wait)
        if args.command == "wait":
            valid_wait(args.seconds)
        store = Store(args.db)
        if args.command == "peer":
            if args.peer_command == "add":
                result = store.add_peer(args.name, args.harness, args.session, args.workspace, args.socket)
            elif args.peer_command == "check":
                result = probe(store.peer(args.name))
            else:
                result = {"peers": store.peers()}
        else:
            if not args.actor:
                raise ValueError("peer_required_use_as_or_HTALK_PEER")
            own = store.peer(args.actor)
            # Available native sender evidence is a mismatch guard, not authentication.
            native_id = os.environ.get("CODEX_THREAD_ID")
            if native_id and own["harness"] == "codex" and own["session_id"] != native_id:
                raise ValueError("actor_conflicts_with_CODEX_THREAD_ID")
            if args.command in ("send", "reply"):
                body = args.message_file.read_text() if args.message_file else args.message
                if args.command == "reply":
                    request = store.get(args.message_id, args.actor)
                    recipient, reply_to, message_id = request["sender"], args.message_id, None
                else:
                    recipient, reply_to, message_id = args.recipient, None, args.id
                result, created = store.save(args.actor, recipient, body, message_id, reply_to)
                if created and not args.no_notify:
                    attempted_notification = True
                    result = store.notify_once(result["id"], notify)
                if args.command == "send" and args.wait:
                    result = store.wait(result["id"], args.actor, args.wait)
                result["created"] = created
                result["next_action"] = "Use wait, inbox, or sent to recover. Never repeat an uncertain notification."
            elif args.command == "wait":
                result = store.wait(args.message_id, args.actor, args.seconds)
            elif args.command == "show":
                result = store.get(args.message_id, args.actor)
            elif args.command == "ack":
                result = store.ack(args.message_id, args.actor)
            elif args.command == "inbox":
                result = store.inbox(args.actor)
            else:
                result = store.sent(args.actor)
        print(json.dumps(result, ensure_ascii=False))
        return 2 if attempted_notification and result.get("submission") != "submitted" else 0
    except KeyboardInterrupt:
        print(json.dumps({"state": "interrupted", "next_action": "Use sent or inbox, then wait/show on the saved ID. Do not resend."}))
        return 130
    except (ValueError, OSError, sqlite3.Error, KeyError, subprocess.SubprocessError) as exc:
        print(json.dumps({"state": "error", "error": str(exc)}))
        return 2


if __name__ == "__main__":
    sys.exit(main())
