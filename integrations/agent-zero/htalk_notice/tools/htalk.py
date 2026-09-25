import asyncio
import os

from helpers.tool import Tool, Response
from usr.plugins.htalk_notice.helpers.receiver import selected_agent


class Htalk(Tool):
    async def execute(self, args=None, **kwargs):
        if not selected_agent(self.agent):
            return Response("htalk is available only to the configured HTALK_CONTEXT main agent.", False)
        if not isinstance(args, list) or not args or not all(isinstance(arg, str) for arg in args):
            return Response("args must be a nonempty list of CLI arguments", False)
        if args[0] == "watch" or (args[0].startswith("-") and args[0] not in ("--help", "-h", "--version", "-V")):
            return Response("Start with a CLI command or --help. The receiver manages watch; use inbox.", False)
        process = None
        try:
            process = await asyncio.create_subprocess_exec(
                os.environ.get("HTALK_BIN") or "htalk", "--as", os.environ["HTALK_PEER"], *args,
                stdin=asyncio.subprocess.DEVNULL,
                stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
            )
            stdout, stderr = await asyncio.wait_for(process.communicate(), timeout=120)
            return Response((stdout or stderr).decode("utf-8", errors="replace"), False)
        except OSError:
            if process is None:
                return Response("Could not start htalk; check HTALK_BIN and PATH. No operation started.", False)
            return Response("Could not complete htalk. Inspect sent or inbox before repeating a write.", False)
        except asyncio.TimeoutError:
            return Response("htalk timed out; an operation may already be saved. Inspect sent or inbox before repeating a write.", False)
        finally:
            if process is not None and process.returncode is None:
                process.terminate()
                try:
                    await asyncio.wait_for(process.wait(), timeout=2)
                except asyncio.TimeoutError:
                    process.kill()
                    await process.wait()
