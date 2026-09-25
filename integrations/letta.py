#!/usr/bin/env python3
"""Managed Letta Code 0.33.0 mailbox session. Linux, Python 3.11+, htalk 0.7.0+."""
import asyncio
import json
import os
from pathlib import Path
import shutil
import sys

from managed_receiver import approve, dispose, emit, entrypoint

INITIAL_PATHS = {"lock", "profile"}
RECOVERY_NOTE = "Conversation discarded; agent memory and settings retained"


def arguments(parser, absolute):
    parser.add_argument("--agent", required=True, help="Dedicated local Letta agent ID")
    parser.add_argument("--letta", default=shutil.which("letta"), type=absolute)


def binding(args):
    if not args.letta or not args.agent.startswith("agent-local-") or "/" in args.agent:
        raise RuntimeError("Provide an installed Letta executable and a dedicated local agent")
    return {"letta": str(args.letta), "agent": args.agent}


def registration(binding):
    return {"name": "htalk", "transport": "stdio", "command": binding["htalk"],
            "args": ["--db", binding["db"], "--as", binding["peer"], "mcp"], "env": {}}


def prepare(args, state):
    profile, binding = args.state / "profile", state["binding"]
    home, work = profile / "home", profile / "work"
    settings = home / ".letta" / "settings.json"
    config = json.loads(settings.read_text())
    selected = [agent for agent in config.get("agents", [])
                if agent.get("agentId") == binding["agent"]
                and agent.get("baseUrl") == "local:" + str(profile / "backend")]
    if len(selected) != 1 or selected[0].get("mcpServers") != [registration(binding)]:
        raise RuntimeError("The dedicated agent's MCP registration must match the receiver exactly")
    for path in (settings, home / ".config/letta/settings.json",
                 work / ".letta/settings.json", work / ".letta/settings.local.json"):
        if path.exists() and json.loads(path.read_text()).get("permissions", {}).get("allow"):
            raise RuntimeError("Remove saved tool allow rules from the dedicated receiver profile")
    # Local memfs remains enabled. It can contain executable mods as well as memory.
    for path in (home / ".letta/extensions",
                 profile / "backend" / "memfs" / binding["agent"] / "memory" / "mods"):
        if path.exists() and any(path.iterdir()):
            raise RuntimeError("The dedicated agent must not load additional mods")
    mods = home / ".letta/mods"
    mods.mkdir(parents=True, exist_ok=True)
    if any(path.name not in ("htalk.ts", "diagnostics") for path in mods.iterdir()):
        raise RuntimeError("The dedicated profile must load only the shipped htalk mod")
    source = Path(__file__).with_name("letta") / "htalk.ts"
    target = mods / "htalk.ts"
    if target.exists() and target.read_bytes() != source.read_bytes():
        raise RuntimeError("Managed htalk mod changed; inspect it before replacing it")
    if not target.exists():
        target.write_bytes(source.read_bytes())
    work.mkdir(parents=True, exist_ok=True)


class Session:
    def __init__(self, args, state):
        self.args, self.state, self.process = args, state, None
        self.turn, self.queue_id, self.stop_reason = None, None, None
        self.dequeued, self.closing, self.failure = False, False, None
        profile, binding = args.state / "profile", state["binding"]
        self.cwd = profile / "work"
        self.env = {key: os.environ[key] for key in ("PATH", "LANG", "TERM") if key in os.environ}
        self.env.update(HOME=str(profile / "home"), XDG_CONFIG_HOME=str(profile / "home/.config"),
            LETTA_HOME=str(profile / "home/.letta"), LETTA_LOCAL_BACKEND_DIR=str(profile / "backend"),
            LETTA_MODS_DIR=str(profile / "home/.letta/mods"), HTALK_LETTA_BIN=binding["letta"],
            HTALK_LETTA_AGENT=binding["agent"], DISABLE_AUTOUPDATER="1",
            LETTA_DISABLE_TELEMETRY="1", DO_NOT_TRACK="1")

    async def inspect(self, *words):
        child = await asyncio.create_subprocess_exec(self.state["binding"]["letta"],
            "--backend", "local", *words, cwd=self.cwd, env=self.env,
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
            start_new_session=True, pass_fds=(self.args.lock_fd,))
        try:
            out, _ = await asyncio.wait_for(child.communicate(), 60)
            if child.returncode:
                raise RuntimeError("Native Letta configuration inspection failed")
            return out.decode().strip()
        finally:
            await dispose(child)

    async def start(self, creating):
        binding = self.state["binding"]
        if await self.inspect("--version") != "0.33.0 (Letta Code)":
            raise RuntimeError("This receiver requires Letta Code 0.33.0")
        servers = json.loads(await self.inspect("mcp", "list", "--agent", binding["agent"]))
        config = json.loads(await self.inspect("mcp", "get", "htalk", "--agent", binding["agent"]))
        if servers != [{"name": "htalk", "transport": "stdio"}] or config != registration(binding):
            raise RuntimeError("Native Letta MCP registration differs from the receiver binding")
        selection = (["--new", "--agent", binding["agent"]] if creating
                     else ["--conversation", self.state["session_id"]])
        self.initial = asyncio.get_running_loop().create_future()
        self.process = await asyncio.create_subprocess_exec(binding["letta"], "--backend", "local",
            *selection, "-p", "", "--input-format", "stream-json", "--output-format", "stream-json",
            "--toolset", "none", "--tools", "htalk", "--permission-mode", "strict",
            "--no-skills", "--reflection-trigger", "off", "--no-system-info-reminder",
            cwd=self.cwd, env=self.env, stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
            stderr=sys.stderr, start_new_session=True, limit=16 * 1024 * 1024,
            pass_fds=(self.args.lock_fd,))
        emit("child_started", kind="letta", pid=self.process.pid)
        self.reader = asyncio.create_task(self.read())
        initial = await asyncio.wait_for(self.initial, 120)
        conversation = initial.get("conversation_id")
        if (initial.get("agent_id") != binding["agent"] or initial.get("tools") != ["htalk"]
                or not isinstance(conversation, str) or not conversation
                or (not creating and conversation != self.state["session_id"])):
            raise RuntimeError("Native Letta session identity or tool surface changed")
        self.state["session_id"] = conversation

    async def send(self, value):
        self.process.stdin.write((json.dumps(value) + "\n").encode())
        await self.process.stdin.drain()

    async def permission(self, value):
        request = value.get("request", {})
        data = request.get("input", {})
        args = data.get("args") if isinstance(data, dict) else None
        permitted = (self.state["phase"] == "inflight" and self.turn is not None
                     and request.get("subtype") == "can_use_tool" and request.get("tool_name") == "htalk"
                     and isinstance(data, dict) and set(data) == {"args"}
                     and isinstance(args, list) and all(isinstance(word, str) for word in args))
        allowed = await approve(permitted, self.args.allow_mail, args, request.get("tool_call_id"))
        await self.send({"type": "control_response", "response": {"request_id": value["request_id"],
            "response": {"behavior": "allow" if allowed else "deny", "message": "Managed receiver decision"}}})

    async def read(self):
        try:
            while line := await self.process.stdout.readline():
                value = json.loads(line)
                kind = value.get("type")
                if kind == "system" and value.get("subtype") == "init":
                    if self.initial.done():
                        raise RuntimeError("Duplicate native initialization")
                    self.initial.set_result(value)
                elif kind == "control_request":
                    await self.permission(value)
                elif kind in ("queue_item_enqueued", "queue_batch_dequeued"):
                    if self.turn is None or value.get("session_id") != self.state["binding"]["agent"]:
                        raise RuntimeError("Unexpected native queue event")
                    if kind == "queue_item_enqueued":
                        if self.queue_id is not None or value.get("source") != "user" or value.get("queue_len") != 1:
                            raise RuntimeError("Unexpected additional native input")
                        self.queue_id = value["item_id"]
                    else:
                        if (self.dequeued or value.get("item_ids") != [self.queue_id]
                                or value.get("merged_count") != 1 or value.get("queue_len_after") != 0):
                            raise RuntimeError("Native input was merged or replaced")
                        self.dequeued = True
                elif kind == "message" and value.get("message_type") == "stop_reason":
                    if self.turn is None:
                        raise RuntimeError("Native output outside a dispatched turn")
                    self.stop_reason = value.get("stop_reason")
                elif kind == "result":
                    if (self.turn is None or self.turn.done() or not self.dequeued
                            or value.get("conversation_id") != self.state["session_id"]
                            or value.get("agent_id") != self.state["binding"]["agent"]):
                        raise RuntimeError("Uncorrelated native turn result")
                    self.turn.set_result(self.stop_reason if value.get("subtype") == "success" else "error")
                emit("native_event", kind=kind, subtype=value.get("subtype"),
                     message_type=value.get("message_type"), stop_reason=value.get("stop_reason"))
            if not self.closing:
                raise RuntimeError("Native Letta connection closed")
        except Exception as error:
            self.failure = error
            for future in (self.initial, self.turn):
                if future is not None and not future.done():
                    future.set_exception(error)

    async def prompt(self, text):
        if self.reader.done() or self.failure:
            raise RuntimeError("Native Letta connection is no longer live")
        self.turn = asyncio.get_running_loop().create_future()
        self.queue_id, self.stop_reason, self.dequeued = None, None, False
        try:
            await self.send({"type": "user", "message": {"role": "user", "content": text}})
            return await self.turn
        finally:
            self.turn = None

    async def close(self):
        if self.process is None:
            return
        self.closing = True
        # Letta emits result before its post-turn memory sync. Give EOF time to
        # finish that native cleanup; a forced stop requires operator inspection.
        try:
            if self.process.returncode is None:
                self.process.stdin.close()
                await asyncio.wait_for(self.process.wait(), 15)
            await self.reader
            if self.process.returncode or self.failure:
                raise RuntimeError("Letta did not close cleanly; inspect the saved dispatch")
        finally:
            await dispose(self.process)


if __name__ == "__main__":
    entrypoint(sys.modules[__name__])
