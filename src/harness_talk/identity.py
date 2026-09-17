"""Recognize the Claude Code session whose tool command runs htalk.

The result only selects a default peer name. Every piece of evidence is readable
and forgeable by the same OS account, so it is not authentication."""
import json
import os
from pathlib import Path
import uuid

MAX_DEPTH = 64
CLIENTS = ("claude", "codex", "opencode")


def process(proc, pid):
    """Return comm, parent PID and start time from /proc/PID/stat."""
    text = (proc / str(pid) / "stat").read_text()
    # comm may contain spaces and parentheses; the fields after the last ")" cannot.
    head, _, tail = text.rpartition(")")
    fields = tail.split()
    return head.partition("(")[2], int(fields[1]), fields[19]


def looks_like_client(proc, pid, comm):
    # The kernel truncates comm to 15 bytes, so compare prefixes of comm and the executable name.
    name = os.path.basename(os.readlink(proc / str(pid) / "exe"))
    return comm.startswith(CLIENTS) or name.startswith(CLIENTS)


def unrecognized(reason):
    return {"harness": "claude", "status": "unrecognized", "reason": reason}


def claude_session(environ=None, proc=Path("/proc"), sessions=None, pid=None):
    """Return the recognized session_id and workspace, or the first failed condition as reason."""
    environ = os.environ if environ is None else environ
    session_id, claude_pid = environ.get("CLAUDE_CODE_SESSION_ID"), environ.get("CLAUDE_PID")
    if not session_id or not claude_pid:
        return unrecognized("claude_session_variables_absent")
    try:
        valid = str(uuid.UUID(session_id)) == session_id and claude_pid.isascii() and claude_pid.isdigit()
    except ValueError:
        valid = False
    if not valid:
        return unrecognized("claude_session_variables_invalid")
    claude_pid = int(claude_pid)
    # A nested Codex command inherits Claude's variables; see the documented tmux limitation.
    if environ.get("CODEX_THREAD_ID"):
        return unrecognized("codex_thread_id_present")
    try:
        started = process(proc, claude_pid)[2]
    except (OSError, ValueError, IndexError):
        return unrecognized("claude_process_unavailable")
    try:
        sessions = Path.home() / ".claude/sessions" if sessions is None else sessions
        metadata = json.loads((sessions / f"{claude_pid}.json").read_text())
    except (OSError, RuntimeError, ValueError):
        return unrecognized("claude_session_metadata_unavailable")
    if not isinstance(metadata, dict):
        return unrecognized("claude_session_metadata_mismatch")
    start = metadata.get("procStart")
    if (metadata.get("sessionId") != session_id or type(metadata.get("pid")) is not int
            or metadata["pid"] != claude_pid or type(start) not in (str, int) or str(start) != started):
        return unrecognized("claude_session_metadata_mismatch")
    between = []
    try:
        parent = process(proc, os.getpid() if pid is None else pid)[1]
        while parent != claude_pid:
            if parent <= 0 or len(between) == MAX_DEPTH:
                return unrecognized("claude_pid_not_an_ancestor")
            comm, grandparent, _ = process(proc, parent)
            between.append((parent, comm))
            parent = grandparent
        # Executables are inspected only below Claude, where they share its owner.
        if any(looks_like_client(proc, *row) for row in between):
            return unrecognized("nested_client_process")
    except (OSError, ValueError, IndexError):
        return unrecognized("process_ancestry_unavailable")
    workspace = metadata.get("cwd")
    return {"harness": "claude", "status": "recognized", "session_id": session_id,
            "workspace": workspace if isinstance(workspace, str) else None}
