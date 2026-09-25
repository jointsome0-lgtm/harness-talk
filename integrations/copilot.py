"""Copilot 1.0.88 notification hooks and one native background waiter. POSIX."""
from contextlib import contextmanager
import fcntl
import json
import os
from pathlib import Path
import selectors
import shlex
import signal
import subprocess
import sys


def root():
    path = Path(os.environ["HTALK_COPILOT_STATE"])
    if not path.is_absolute() or not path.is_dir():
        raise ValueError("HTALK_COPILOT_STATE must be a private, fresh directory for this session")
    return path


@contextmanager
def state():
    with (root() / "state.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        path = root() / "state.json"
        data = json.loads(path.read_text()) if path.exists() else {}
        before = json.dumps(data)
        yield data
        if json.dumps(data) == before:
            return
        temporary = path.with_suffix(".tmp")
        temporary.write_text(json.dumps(data))
        temporary.replace(path)


def arm_instruction():
    command = shlex.join([sys.executable, str(Path(__file__).resolve()), "wait"])
    return (f"Run {command} once with the native bash tool using mode async. "
            "Leave it running in the background and finish your turn. It waits for htalk mail; "
            "do not poll it or start a second waiter. Keep normal tool approval rules.")


def hook(kind):
    # An installed hook is inert in sessions which did not opt in.
    if not all(os.environ.get(k) for k in ("HTALK_PEER", "HTALK_DB", "HTALK_COPILOT_STATE")):
        print("{}"); return
    event = json.load(sys.stdin)
    session_id = event.get("sessionId")
    if not isinstance(session_id, str) or not session_id:
        raise ValueError("missing native sessionId")
    output = {}
    with state() as saved:
        binding = {"session_id": session_id, "peer": os.environ["HTALK_PEER"],
                   "db": str(Path(os.environ["HTALK_DB"]).resolve())}
        if kind == "start" and not saved:
            saved.update(binding, offered=[], pending=None, closed=False)
            output = {"additionalContext": "htalk receiver enabled. " + arm_instruction()}
        elif any(saved.get(k) != v for k, v in binding.items()):
            raise ValueError("receiver state belongs to a different session or mailbox")
        elif kind == "end":
            saved["closed"] = True
        elif kind == "notice" and not saved["closed"] and event.get("notification_type") in (
                "shell_completed", "shell_detached_completed") and saved["pending"]:
            output = {"additionalContext": saved.pop("pending") + "\nAfter handling the saved notice, " + arm_instruction()}
            saved["pending"] = None
    print(json.dumps(output))


def wait():
    with (root() / "wait.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        with state() as saved:
            if not saved or saved.get("closed") or saved.get("pending"):
                raise ValueError("session is closed, unbound or awaiting notification delivery")
            peer, database = saved["peer"], saved["db"]
        child = subprocess.Popen([os.environ.get("HTALK_BIN", "htalk"), "--db", database,
                                  "--as", peer, "watch"], stdout=subprocess.PIPE,
                                 stderr=subprocess.DEVNULL, bufsize=0)
        try:
            with selectors.DefaultSelector() as selector:
                selector.register(child.stdout, selectors.EVENT_READ)
                while True:
                    with state() as saved:
                        if saved["closed"]:
                            return
                    if not selector.select(1):
                        continue
                    line = child.stdout.readline()
                    if not line:
                        raise RuntimeError("htalk watch exited")
                    event = json.loads(line)
                    if event.get("event") == "ready" and event.get("peer") == peer:
                        continue
                    if event.get("event") != "message" or not isinstance(event.get("id"), str) or not isinstance(event.get("notification"), str):
                        raise ValueError("unexpected watch event")
                    with state() as saved:
                        if saved["closed"]:
                            return
                        if event["id"] in saved["offered"]:
                            continue
                        saved["offered"].append(event["id"])
                        saved["pending"] = event["notification"]
                    print("htalk notice ready; the notification hook will supply its saved ID.")
                    return
        finally:
            if child.poll() is None:
                child.terminate()
            try:
                child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                child.kill(); child.wait()


if __name__ == "__main__":
    os.umask(0o077)
    def terminate(signum, _frame):
        # Python's default SIGTERM skips the waiter's child-reaping finally.
        raise SystemExit(128 + signum)
    signal.signal(signal.SIGTERM, terminate)
    try:
        if sys.argv[1:] == ["wait"]:
            wait()
        elif len(sys.argv) == 2 and sys.argv[1] in ("start", "notice", "end"):
            hook(sys.argv[1])
        else:
            raise ValueError("usage: copilot.py start|notice|end|wait")
    except (OSError, ValueError, RuntimeError, KeyError) as error:
        print(f"htalk receiver stopped: {type(error).__name__}. Do not rerun this waiter; tell the user. "
              "Check configuration and saved mail before a manual restart.", file=sys.stderr)
        raise SystemExit(1)
