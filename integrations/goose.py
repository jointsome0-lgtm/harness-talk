#!/usr/bin/env python3
"""Managed Goose 1.52.0 mailbox session. Linux, Python 3.11+, htalk 0.7.0+."""
import asyncio
import json
import os
import shutil
import sys

from managed_receiver import approve, dispose, emit, entrypoint, save

INITIAL_PATHS = {"lock"}
RECOVERY_NOTE = "Conversation discarded; old native history remains on disk"
PROFILE = {"extensions": {name: {"enabled": False, "type": "platform", "name": name}
           for name in ("developer", "skills", "tom", "summon", "scheduler",
                        "apps", "analyze", "extensionmanager")}}


def arguments(parser, absolute):
    parser.add_argument("--workspace", required=True, type=absolute)
    parser.add_argument("--goose", default=shutil.which("goose"), type=absolute)


def binding(args):
    if not args.goose:
        raise RuntimeError("Provide an installed Goose executable")
    return {"goose": str(args.goose), "workspace": str(args.workspace)}


def prepare(args, state):
    profile = args.state / "profile"
    config = profile / "config" / "config.yaml"
    if not config.exists():
        if state["phase"] != "new" or profile.exists():
            raise RuntimeError("Managed profile is missing or was not created by this adapter")
        config.parent.mkdir(parents=True)
        # JSON is valid YAML and needs no extra parser dependency.
        save(config, PROFILE)
    if json.loads(config.read_text()).get("extensions") != PROFILE["extensions"]:
        raise RuntimeError("Managed profile extensions changed; inspect before starting")


class ACP:
    def __init__(self, process, state, automatic_mail_permissions):
        self.process, self.state = process, state
        self.automatic_mail_permissions = automatic_mail_permissions
        self.sequence, self.pending = 0, {}
        self.reader = asyncio.create_task(self.read())

    async def send(self, value):
        self.process.stdin.write((json.dumps({"jsonrpc": "2.0", **value}) + "\n").encode())
        await self.process.stdin.drain()

    async def request(self, method, params):
        if self.reader.done():
            raise RuntimeError("ACP connection is no longer live")
        self.sequence += 1
        number = self.sequence
        future = asyncio.get_running_loop().create_future()
        self.pending[number] = future
        emit("acp_request", method=method, id=number,
             session_id=params.get("sessionId"))
        try:
            await self.send({"id": number, "method": method, "params": params})
            # A human approval or a peer wait may legitimately take a long time.
            # Explicit interruption preserves inflight state for inspection.
            return await asyncio.wait_for(future, None if method == "session/prompt" else 120)
        finally:
            self.pending.pop(number, None)

    async def permission(self, params):
        tool = params.get("toolCall", {})
        args = tool.get("rawInput", {}).get("args", [])
        permitted = (self.state["phase"] == "inflight"
                     and params.get("sessionId") == self.state.get("session_id")
                     and tool.get("title") == "htalk: htalk"
                     and isinstance(args, list) and all(isinstance(v, str) for v in args))
        allowed = await approve(permitted, self.automatic_mail_permissions, args,
                                tool.get("toolCallId"))
        kind = "allow_once" if allowed else "reject_once"
        option = next((v for v in params.get("options", []) if v["kind"] == kind), None)
        return ({"outcome": {"outcome": "selected", "optionId": option["optionId"]}}
                if option else {"outcome": {"outcome": "cancelled"}})

    async def read(self):
        try:
            while line := await self.process.stdout.readline():
                value = json.loads(line)
                if "method" not in value:
                    future = self.pending.get(value.get("id"))
                    if future is not None and not future.done():
                        if "error" in value:
                            emit("acp_error", code=value["error"].get("code"))
                            future.set_exception(RuntimeError("ACP request failed"))
                        else:
                            future.set_result(value.get("result", {}))
                elif "id" in value:
                    if value["method"] == "session/request_permission":
                        answer = await self.permission(value.get("params", {}))
                        await self.send({"id": value["id"], "result": answer})
                    else:
                        await self.send({"id": value["id"], "error": {
                            "code": -32601, "message": "Unsupported client method"}})
                else:
                    params = value.get("params", {})
                    update = params.get("update", {})
                    emit("acp_update", phase=self.state["phase"], method=value["method"],
                         session_id=params.get("sessionId"), kind=update.get("sessionUpdate"),
                         tool_call_id=update.get("toolCallId"), status=update.get("status"))
            raise RuntimeError("ACP connection closed")
        except Exception as error:
            for future in self.pending.values():
                if not future.done():
                    future.set_exception(error)


class Session:
    def __init__(self, args, state):
        self.args, self.state, self.process = args, state, None

    async def start(self, creating):
        binding = self.state["binding"]
        self.process = await asyncio.create_subprocess_exec(
            binding["goose"], "acp", cwd=binding["workspace"], stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE, stderr=sys.stderr, start_new_session=True,
            limit=16 * 1024 * 1024, pass_fds=(self.args.lock_fd,),
            env={**os.environ, "GOOSE_PATH_ROOT": str(self.args.state / "profile"), "GOOSE_MODE": "approve"})
        emit("child_started", kind="goose", pid=self.process.pid)
        self.acp = ACP(self.process, self.state, self.args.allow_mail)
        self.reader = self.acp.reader
        initial = await self.acp.request("initialize", {"protocolVersion": 1,
            "clientCapabilities": {}, "clientInfo": {"name": "htalk-goose", "version": "1"}})
        if (initial.get("agentInfo", {}).get("version") != "1.52.0"
                or not initial.get("agentCapabilities", {}).get("loadSession")):
            raise RuntimeError("This receiver requires Goose 1.52.0 with session loading")
        if creating:
            result = await self.acp.request("session/new", {"cwd": binding["workspace"], "mcpServers": [{
                "name": "htalk", "command": binding["htalk"], "args": ["--db", binding["db"],
                "--as", binding["peer"], "mcp"], "env": []}]})
            self.state["session_id"] = result["sessionId"]
        else:
            await self.acp.request("session/load", {"sessionId": self.state["session_id"],
                                  "cwd": binding["workspace"], "mcpServers": []})

    async def prompt(self, text):
        result = await self.acp.request("session/prompt", {"sessionId": self.state["session_id"],
            "prompt": [{"type": "text", "text": text}]})
        return result.get("stopReason")

    async def close(self):
        await dispose(self.process)


if __name__ == "__main__":
    entrypoint(sys.modules[__name__])
