"""Read native session metadata. Discovery never registers or wakes a peer."""
from collections import Counter
from contextlib import closing
import json
import os
from pathlib import Path
import subprocess
import sqlite3
import stat
import time
import uuid

from .adapters import codex_home, codex_rpc, codex_state_path, owned_socket


def address(session_id, workspace):
    session_id = str(uuid.UUID(session_id))
    if not isinstance(workspace, str) or not Path(workspace).is_absolute():
        raise ValueError("invalid_workspace")
    return session_id, workspace


def discover_claude():
    source = {"harness": "claude", "source": "claude_agents", "status": "ok"}
    sessions = []
    try:
        result = subprocess.run(["claude", "agents", "--json"], capture_output=True,
                                text=True, check=True, timeout=15)
        rows = json.loads(result.stdout)
        if not isinstance(rows, list):
            raise ValueError("invalid_claude_agents_response")
        identities = Counter(row.get("sessionId") for row in rows
                             if isinstance(row, dict) and isinstance(row.get("sessionId"), str))
        rejected = 0
        for row in rows:
            try:
                session_id, workspace = address(row["sessionId"], row["cwd"])
                pid = row["pid"]
                if identities[row["sessionId"]] != 1 or type(pid) is not int or pid <= 0:
                    raise ValueError("ambiguous_claude_identity")
                metadata = json.loads((Path.home() / ".claude/sessions" / f"{pid}.json").read_text())
                if metadata.get("sessionId") != session_id or metadata.get("cwd") != workspace:
                    raise ValueError("claude_identity_changed")
                owned_socket(metadata["messagingSocketPath"])
                sessions.append({"harness": "claude", "session_id": session_id,
                                 "workspace": workspace, "runtime_status": "running",
                                 "source": "claude_agents", "pid": pid})
            except (OSError, ValueError, KeyError, TypeError, AttributeError):
                rejected += 1
        if rejected:
            source.update(status="partial", rejected=rejected,
                          detail="Some live records could not be verified; run discovery again to refresh.")
    except (OSError, ValueError, subprocess.SubprocessError) as exc:
        source.update(status="unavailable", detail=type(exc).__name__)
    return {"sessions": sessions, "sources": [source]}


def default_codex_socket():
    return codex_home() / "app-server-control/app-server-control.sock"


def discover_codex_writers(lock_table=Path("/proc/locks")):
    """Linux kernel lock evidence for native CLI threads, without taking a lock."""
    directory = codex_home() / "thread-writer-locks"
    source = {"harness": "codex", "source": "codex_writer_locks", "status": "ok",
              "path": str(directory), "scope": "CLI writers visible in this Linux process namespace."}
    sessions, files, held = [], {}, {}
    try:
        for path in directory.iterdir():
            if path.suffix != ".lock" or path.name.startswith("."):
                continue
            try:
                ident = str(uuid.UUID(path.stem))
                info = path.lstat()
            except (OSError, ValueError):
                continue
            if stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid():
                files[(os.major(info.st_dev), os.minor(info.st_dev), info.st_ino)] = ident
        for line in lock_table.read_text().splitlines():
            fields = line.split()
            # Ignore blocked lock requests ("->"), read locks and unrelated lock types.
            if len(fields) < 6 or fields[1:4] != ["FLOCK", "ADVISORY", "WRITE"]:
                continue
            major, minor, inode = fields[5].split(":")
            key = (int(major, 16), int(minor, 16), int(inode))
            if key in files and int(fields[4]) > 0:
                held[files[key]] = int(fields[4])
        if held:
            metadata = codex_state_path()
            with closing(sqlite3.connect(metadata.as_uri() + "?mode=ro", uri=True, timeout=3)) as db:
                for ident, pid in held.items():
                    row = db.execute("SELECT id, cwd, archived, source FROM threads WHERE id=?", (ident,)).fetchone()
                    if row is None:
                        source.update(status="partial", detail="Some held writers have no saved address yet.")
                        continue
                    if row[2] != 0 or row[3] != "cli":
                        continue
                    session_id, workspace = address(row[0], row[1])
                    sessions.append({"harness": "codex", "session_id": session_id, "workspace": workspace,
                                     "runtime_status": "writer_active", "runtime_reason": "kernel_writer_lock",
                                     "source": "codex_writer_locks", "pid": pid, "socket": None})
    except (OSError, ValueError, sqlite3.Error) as exc:
        source.update(status="partial" if sessions else "unavailable", detail=type(exc).__name__)
    return {"sessions": sessions, "sources": [source]}


def discover_codex(paths=None):
    sessions, sources = [], []
    paths = [default_codex_socket()] if paths is None else paths
    for path in dict.fromkeys(str(Path(p).expanduser().resolve()) for p in paths):
        source = {"harness": "codex", "source": "codex_app_server", "socket": path, "status": "ok"}
        sources.append(source)
        count, rejected = 0, 0
        try:
            with codex_rpc({"socket": path}) as rpc:
                deadline = time.monotonic() + 15
                cursor, seen_cursors, seen_ids = None, set(), set()
                while True:
                    if time.monotonic() >= deadline:
                        raise TimeoutError("codex_discovery_time_limit")
                    page = rpc.call("thread/loaded/list", {"cursor": cursor, "limit": 100})
                    if not isinstance(page, dict) or not isinstance(page.get("data"), list):
                        raise ValueError("invalid_loaded_threads_response")
                    for ident in page["data"]:
                        if len(seen_ids) >= 200 or time.monotonic() >= deadline:
                            raise TimeoutError("codex_discovery_limit")
                        try:
                            ident = str(uuid.UUID(ident))
                            if ident in seen_ids:
                                continue
                            seen_ids.add(ident)
                            thread = rpc.call("thread/read", {"threadId": ident, "includeTurns": False})["thread"]
                            session_id, workspace = address(thread["id"], thread["cwd"])
                            if session_id != ident:
                                raise ValueError("codex_identity_changed")
                            status = thread["status"]["type"]
                            if status == "notLoaded":
                                continue  # It was unloaded between list and read.
                            if thread.get("canAcceptDirectInput") is False:
                                continue  # Internal workers belong to their controlling client.
                            if status not in ("idle", "active", "systemError"):
                                raise ValueError("unknown_codex_runtime_status")
                            sessions.append({"harness": "codex", "session_id": session_id,
                                             "workspace": workspace, "runtime_status": status,
                                             "source": "codex_app_server", "socket": path})
                            count += 1
                        except (ValueError, KeyError, TypeError, AttributeError):
                            rejected += 1
                    cursor = page.get("nextCursor")
                    if cursor is None:
                        break
                    if not isinstance(cursor, str) or cursor in seen_cursors or len(seen_cursors) >= 20:
                        raise ValueError("invalid_discovery_cursor")
                    seen_cursors.add(cursor)
                if rejected:
                    source.update(status="partial", rejected=rejected,
                                  detail="Some loaded records could not be verified.")
        except (OSError, ValueError, KeyError, TypeError, AttributeError) as exc:
            source.update(status="partial" if count else "unavailable", detail=str(exc) if
                          isinstance(exc, (TimeoutError, ValueError)) else type(exc).__name__)
        if source["status"] == "unavailable":
            source["next_action"] = ("Check the running Codex app-server socket and permissions, or pass --codex-socket. "
                                     "Embedded clients without a socket are outside this source's coverage.")
    return {"sessions": sessions, "sources": sources}


def discover(harness=None, workspace=None, codex_sockets=None, opencode_urls=None):
    sessions, sources = [], []
    if workspace is not None:
        workspace = str(Path(workspace).expanduser().resolve())
    if harness in (None, "claude"):
        result = discover_claude()
        sessions.extend(result["sessions"])
        sources.extend(result["sources"])
    if harness in (None, "codex"):
        result = discover_codex(codex_sockets)
        sessions.extend(result["sessions"])
        sources.extend(result["sources"])
        if codex_sockets is None:
            writers = discover_codex_writers()
            known = {(row["session_id"], row["workspace"]) for row in result["sessions"]}
            sessions.extend(row for row in writers["sessions"] if (row["session_id"], row["workspace"]) not in known)
            sources.extend(writers["sources"])
    if harness in (None, "opencode"):
        from .opencode import discover as discover_opencode
        result = discover_opencode(opencode_urls, workspace=workspace)
        sessions.extend(result["sessions"])
        sources.extend(result["sources"])
    if workspace is not None:
        sessions = [row for row in sessions if row["workspace"] == workspace]
    sessions.sort(key=lambda row: (row["harness"], row["workspace"], row["session_id"]))
    return {"sessions": sessions, "sources": sources,
            "next_action": "Choose an exact session address and register it with peer add. Discovery does not register or notify anyone.",
            "scope": "Current local client environment and the listed endpoints only. Unavailable sources do not prove there are no sessions."}
