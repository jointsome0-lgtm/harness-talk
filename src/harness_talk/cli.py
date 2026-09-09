"""htalk: durable local messages with optional client notifications."""
import argparse
import json
import os
from pathlib import Path
import shlex
import sqlite3
import subprocess
import sys
import uuid

from . import __version__
from .adapters import notify, probe
from .discovery import discover
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
    add.add_argument("--harness", choices=("codex", "claude", "opencode"), required=True)
    add.add_argument("--session", required=True)
    add.add_argument("--workspace", required=True)
    add.add_argument("--socket", help="Optional Codex standalone app-server socket; default uses native codex queue.")
    add.add_argument("--url", help="Local OpenCode server URL; defaults to http://127.0.0.1:4096.")
    peer.add_parser("list")
    find = peer.add_parser("discover", help="Find native session addresses without registering or messaging them.")
    find.add_argument("--harness", choices=("codex", "claude", "opencode"))
    find.add_argument("--workspace", help="Only return sessions in this exact workspace.")
    find.add_argument("--codex-socket", action="append", help="Inspect this running app-server socket; repeat for several servers.")
    find.add_argument("--opencode-url", action="append", help="Inspect this local OpenCode server; repeat for several servers.")
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


def command(args, *parts, actor=True):
    words = ["htalk", "--db", str(args.db.expanduser().resolve())]
    if actor and args.actor:
        words += ["--as", args.actor]
    return shlex.join([*words, *parts])


def message_actions(args, message):
    """CLI-only guidance; stored messages and marks remain unchanged."""
    recovery = {"show": command(args, "show", message["id"])}
    if message["sender"] == args.actor and message["in_reply_to"] is None:
        if message["reply"]:
            answer = message["reply"]
            recovery["show_reply"] = command(args, "show", answer["id"])
            action = "Read the saved answer with recovery.show_reply."
            if answer["ack_at"] is None:
                recovery["ack_after_reading"] = command(args, "ack", answer["id"])
                action += " After reading, use recovery.ack_after_reading."
        else:
            recovery["wait"] = command(args, "wait", message["id"], "--seconds", "45")
            action = "Use recovery.wait to wait again on this saved request, or recovery.show to inspect it."
    elif message["recipient"] == args.actor:
        action = "Read the message with recovery.show."
        if message["ack_at"] is None:
            recovery["ack_after_reading"] = command(args, "ack", message["id"])
            action += " After reading, use recovery.ack_after_reading."
        if message["in_reply_to"] is None and message["reply"] is None:
            action += " Acknowledging a question leaves it open until you reply."
    else:
        action = "Inspect the saved answer with recovery.show; the recipient can retrieve it from their inbox."
    message["recovery"] = recovery
    message["next_action"] = action + " Never repeat an uncertain notification."


def main(argv=None):
    args = parser().parse_args(argv)
    saved_id = None
    own = None
    try:
        attempted_notification = False
        os.umask(0o077)
        if args.command == "peer" and args.peer_command == "discover":
            if args.codex_socket and args.harness not in (None, "codex"):
                raise ValueError("codex_socket_requires_codex_discovery")
            if args.opencode_url and args.harness not in (None, "opencode"):
                raise ValueError("opencode_url_requires_opencode_discovery")
            result = discover(args.harness, args.workspace, args.codex_socket, args.opencode_url)
            print(json.dumps(result, ensure_ascii=False))
            return 2 if all(source["status"] == "unavailable" for source in result["sources"]) else 0
        if args.command == "send":
            valid_wait(args.wait)
        if args.command == "wait":
            valid_wait(args.seconds)
        store = Store(args.db)
        if args.command == "peer":
            if args.peer_command == "add":
                result = store.add_peer(args.name, args.harness, args.session, args.workspace, args.socket, url=args.url)
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
                saved_id = result["id"]
                if created and not args.no_notify:
                    attempted_notification = True
                    result = store.notify_once(result["id"], notify)
                if args.command == "send" and args.wait:
                    result = store.wait(result["id"], args.actor, args.wait)
                result["created"] = created
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
            if "messages" in result:
                for message in result["messages"]:
                    message_actions(args, message)
            else:
                message_actions(args, result)
        print(json.dumps(result, ensure_ascii=False))
        return 2 if attempted_notification and result.get("submission") != "submitted" else 0
    except KeyboardInterrupt:
        recovery = {"peers": command(args, "peer", "list", actor=False)}
        if own:
            recovery.update(sent=command(args, "sent"), inbox=command(args, "inbox"))
        known_id = saved_id or getattr(args, "message_id", None) or getattr(args, "id", None)
        try:
            known_id = str(uuid.UUID(known_id)) if known_id else None
        except (ValueError, TypeError, AttributeError):
            known_id = None
        if known_id and own:
            recovery["show"] = command(args, "show", known_id)
        print(json.dumps({"state": "interrupted", "message_id": known_id, "recovery": recovery,
                          "persistence": "saved" if saved_id else "unknown",
                          "next_action": "Use the listed recovery commands to inspect the known ID or find saved messages. Do not resend."}))
        return 130
    except (ValueError, OSError, sqlite3.Error, KeyError, subprocess.SubprocessError) as exc:
        error = str(exc)
        result = {"state": "error", "error": error,
                  "recovery": {"peers": command(args, "peer", "list", actor=False)}}
        if error == "actor_conflicts_with_CODEX_THREAD_ID":
            result.update(registered_peer=args.actor, registered_session_id=own["session_id"],
                          current_session_id=native_id)
            result["recovery"]["inbox_in_registered_session"] = command(args, "inbox")
            result["next_action"] = (
                f"Peer {args.actor} belongs to Codex session {own['session_id']}, not current session {native_id}. "
                "Use recovery.peers to find the peer registered to this session, or return to the registered "
                "session before running recovery.inbox_in_registered_session. The binding cannot be reassigned.")
        elif error == "peer_already_has_a_different_address":
            registered = store.peer(args.name)
            result.update(registered_peer=args.name, registered_session_id=registered["session_id"])
            result["next_action"] = (
                f"Peer {args.name} is already bound to {registered['harness']} session {registered['session_id']}. "
                "Inspect recovery.peers. Keep that address for the existing session; a separate session needs a different peer name.")
        elif error == "session_already_has_a_peer_name":
            session_id = args.session if args.harness == "opencode" else str(uuid.UUID(args.session))
            registered = next(peer for peer in store.peers()
                              if peer["harness"] == args.harness and peer["session_id"] == session_id)
            result.update(registered_peer=registered["name"], registered_session_id=registered["session_id"])
            result["next_action"] = (
                f"Session {registered['session_id']} already uses peer {registered['name']}. "
                "Use that name from its registered session. Inspect recovery.peers for the existing immutable addresses.")
        elif error in ("message_id_conflict", "reply_conflict_existing_answer_preserved"):
            known_id = str(uuid.UUID(args.id)) if args.command == "send" else args.message_id
            result["message_id"] = known_id
            result["recovery"]["sent"] = command(args, "sent")
            try:
                store.get(known_id, args.actor)
            except ValueError:
                result["next_action"] = "This ID is already in use and is not readable by this peer. Recover your own outgoing IDs with recovery.sent. Do not resend."
            else:
                result["recovery"]["show"] = command(args, "show", known_id)
                result["next_action"] = "The existing message was preserved. Inspect recovery.show or recover outgoing IDs with recovery.sent. Do not resend."
        else:
            if own:
                result["recovery"].update(sent=command(args, "sent"), inbox=command(args, "inbox"))
                result["next_action"] = "Inspect saved messages with recovery.sent or recovery.inbox. Check registered addresses with recovery.peers. Never repeat an uncertain notification."
            else:
                result["next_action"] = "Check the error and inspect registered addresses with recovery.peers."
        print(json.dumps(result))
        return 2


if __name__ == "__main__":
    sys.exit(main())
