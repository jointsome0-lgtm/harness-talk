#!/usr/bin/env python3
"""Managed Antigravity SDK 0.1.18 mailbox session. Linux, Python 3.11+, htalk 0.7.0+."""
import asyncio
import json
import os
from pathlib import Path
import sys
from urllib.parse import urlsplit
import uuid

from managed_receiver import dispose, emit, entrypoint

INITIAL_PATHS = {"lock"}
RECOVERY_NOTE = "Conversation retired; private SDK session files retained"
TOOL_INSTRUCTION = (
    'Access htalk with call_mcp_tool: ServerName="htalk", ToolName="htalk", '
    'Arguments={"args":[...]}; include the required toolSummary and toolAction. '
    'args is an array of CLI words, without the executable, '
    '--db or --as; the server binds identity and mailbox. Examples: '
    '["show","MESSAGE_ID"], ["ack","MESSAGE_ID"], ["peer","list"], ["sent"], '
    '["send","PEER","--id","UUID","--message","TEXT"], '
    '["reply","REQUEST_ID","--message","TEXT"].\n\n'
)


def arguments(parser, absolute):
    # Resolving a venv's Python symlink would select its base interpreter.
    parser.add_argument("--sdk-python", required=True,
                        type=lambda value: Path(value).expanduser().absolute())
    parser.add_argument("--model", required=True)
    parser.add_argument("--base-url", required=True,
                        help="Local OpenAI-compatible endpoint without credentials")


def binding(args):
    if not args.allow_mail:
        raise RuntimeError("Antigravity requires --allow-mail; its SDK uses a static htalk policy")
    url = urlsplit(args.base_url)
    if (url.scheme != "http" or url.hostname not in ("localhost", "127.0.0.1", "::1")
            or url.username is not None or url.password is not None or url.query or url.fragment):
        raise RuntimeError("Provide a local HTTP endpoint without credentials, query or fragment")
    if not args.sdk_python.is_file() or not args.model.strip():
        raise RuntimeError("Provide an SDK Python executable and a model name")
    return {"sdk_python": str(args.sdk_python), "model": args.model, "base_url": args.base_url}


def prepare(args, state):
    for name in ("home", "work", "sessions", "app-data"):
        (args.state / "profile" / name).mkdir(parents=True, exist_ok=True)


class Session:
    def __init__(self, args, state):
        self.args, self.state, self.process = args, state, None
        self.turn, self.failure, self.closing = None, None, False

    async def start(self, creating):
        binding, profile = self.state["binding"], self.args.state / "profile"
        if creating:
            self.state["session_id"] = str(uuid.uuid4())
        env = {key: os.environ[key] for key in ("PATH", "LANG", "TERM") if key in os.environ}
        env.update(HOME=str(profile / "home"), XDG_CONFIG_HOME=str(profile / "home/.config"),
                   XDG_DATA_HOME=str(profile / "home/.local/share"),
                   NO_PROXY="localhost,127.0.0.1,::1", DO_NOT_TRACK="1")
        self.initial = asyncio.get_running_loop().create_future()
        self.process = await asyncio.create_subprocess_exec(binding["sdk_python"], "-B", "-E", "-s",
            str(Path(__file__).with_name("antigravity") / "worker.py"), cwd=profile / "work", env=env,
            stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE, stderr=sys.stderr,
            start_new_session=True, pass_fds=(self.args.lock_fd,), limit=16 * 1024 * 1024)
        emit("child_started", kind="antigravity_sdk", pid=self.process.pid)
        self.reader = asyncio.create_task(self.read())
        await self.send({"binding": binding, "profile": str(profile), "creating": creating,
                         "session_id": self.state["session_id"]})
        initial = await asyncio.wait_for(self.initial, 120)
        if (initial.get("configured_session_id") != self.state["session_id"]
                or initial.get("sdk_version") != "0.1.18"):
            raise RuntimeError("Native SDK configuration differs from the saved binding")

    async def send(self, value):
        self.process.stdin.write((json.dumps(value) + "\n").encode())
        await self.process.stdin.drain()

    async def read(self):
        try:
            while line := await self.process.stdout.readline():
                value = json.loads(line)
                kind = value.get("event")
                if kind == "configured" and not self.initial.done():
                    self.initial.set_result(value)
                elif kind == "result":
                    if (self.turn is None or self.turn.done()
                            or value.get("message_id") != self.state["pending"]["id"]
                            or value.get("session_id") != self.state["session_id"]):
                        raise RuntimeError("Uncorrelated native SDK result")
                    self.turn.set_result("end_turn" if value.get("complete") is True else "error")
                elif kind == "error":
                    raise RuntimeError(value.get("detail", "Native SDK failed"))
                else:
                    raise RuntimeError("Unexpected native SDK event")
                emit("native_event", kind=kind)
            if not self.closing:
                raise RuntimeError("Native SDK connection closed")
        except Exception as error:
            self.failure = error
            for future in (self.initial, self.turn):
                if future is not None and not future.done():
                    future.set_exception(error)

    async def prompt(self, text):
        if self.reader.done() or self.failure:
            raise RuntimeError("Native SDK connection is no longer live")
        self.turn = asyncio.get_running_loop().create_future()
        try:
            await self.send({"message_id": self.state["pending"]["id"],
                             "text": TOOL_INSTRUCTION + text})
            return await self.turn
        finally:
            self.turn = None

    async def close(self):
        if self.process is None:
            return
        self.closing = True
        try:
            if self.process.returncode is None:
                self.process.stdin.close()
                await asyncio.wait_for(self.process.wait(), 15)
            await self.reader
            if self.process.returncode or self.failure:
                raise RuntimeError("Antigravity did not close cleanly; inspect the saved dispatch")
        finally:
            await dispose(self.process)


if __name__ == "__main__":
    entrypoint(sys.modules[__name__])
