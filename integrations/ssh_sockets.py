"""Prepare two private reverse-SSH sockets after the previous tunnel has exited."""
import argparse
import errno
import fcntl
import json
import os
from pathlib import Path
import socket
import stat


def prepare(directory):
    path = Path(directory)
    if not path.is_absolute() or path.resolve(strict=True) != path:
        raise ValueError("Use an absolute directory without symlinks")
    metadata = path.stat()
    if not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid != os.getuid() or metadata.st_mode & 0o077:
        raise ValueError("Socket directory must be private and owned by this account")
    with os.fdopen(os.open(path / ".prepare.lock", os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW,
                           0o600), "r+") as lock:
        metadata = os.fstat(lock.fileno())
        if (not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid()
                or metadata.st_nlink != 1 or metadata.st_mode & 0o077):
            raise ValueError("Invalid preparation lock")
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        names = ("mcp.sock", "watch.sock")
        if set(os.listdir(path)) - {*names, ".prepare.lock"}:
            raise ValueError("Use a directory containing only this bridge's two sockets")

        def stale(name):
            file = path / name
            try:
                item = file.lstat()
            except FileNotFoundError:
                return None
            if not stat.S_ISSOCK(item.st_mode) or item.st_uid != os.getuid():
                raise ValueError(f"Refusing non-socket or foreign socket: {name}")
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as probe:
                probe.settimeout(1)
                try:
                    probe.connect(str(file))
                except OSError as error:
                    if error.errno != errno.ECONNREFUSED:
                        raise RuntimeError(f"Socket state uncertain: {name}") from error
                else:
                    raise RuntimeError(f"Socket still accepts connections: {name}")
            return item.st_dev, item.st_ino, item.st_ctime_ns

        # Check both before deleting either. Never replace a live endpoint.
        checked = {name: stale(name) for name in names}
        removed = []
        for name, identity in checked.items():
            if identity is not None:
                if stale(name) != identity:
                    raise RuntimeError(f"Socket changed during preparation: {name}")
                (path / name).unlink()
                removed.append(name)
        return {"ready": True, "removed": removed}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", help="dedicated private directory on the SSH server")
    args = parser.parse_args()
    try:
        print(json.dumps(prepare(args.directory)))
    except (OSError, ValueError, RuntimeError) as error:
        parser.exit(1, f"ssh socket preparation refused: {error}\n")
