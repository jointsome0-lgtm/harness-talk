"""Private SDK host: isolate environment, retain the receiver lock, use native Agent."""
import asyncio
import json
from pathlib import Path
import signal
import sys

from google.antigravity import Agent, types

from receiver_config import configuration


def emit(event, **fields):
    print(json.dumps({"event": event, **fields}), flush=True)


async def run():
    reader = asyncio.StreamReader(limit=16 * 1024 * 1024)
    protocol = asyncio.StreamReaderProtocol(reader)
    transport, _ = await asyncio.get_running_loop().connect_read_pipe(lambda: protocol, sys.stdin)
    try:
        settings = json.loads(await reader.readline())
        binding, profile = settings["binding"], Path(settings["profile"])
        config = configuration(profile, binding["htalk"], binding["db"], binding["peer"],
            binding["model"], binding["base_url"], settings["session_id"], settings["creating"])
        async with Agent(config) as agent:
            # SDK 0.1.18 exposes the native ID after the first completed turn.
            # CREATE_ONLY/RESUME use the configured ID during the handshake.
            emit("configured", configured_session_id=settings["session_id"], sdk_version="0.1.18")
            while line := await reader.readline():
                request = json.loads(line)
                history = agent.conversation.history
                previous_step = history[-1].id if history else None
                response = await agent.chat(request["text"])
                try:
                    await response.text()
                except asyncio.CancelledError:
                    await response.cancel()
                    raise
                history = agent.conversation.history
                last = history[-1] if history else None
                complete = (agent.conversation.connection.is_idle
                    and response.stop_reason == types.StopReason.UNSPECIFIED
                    and last is not None and last.id != previous_step
                    and last.type == types.StepType.TEXT_RESPONSE
                    and last.status == types.StepStatus.DONE
                    and last.is_complete_response and not last.error)
                emit("result", message_id=request["message_id"],
                     session_id=agent.conversation_id, complete=complete)
    finally:
        transport.close()


async def main():
    task = asyncio.current_task()
    asyncio.get_running_loop().add_signal_handler(signal.SIGTERM, task.cancel)
    await run()


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except (KeyboardInterrupt, asyncio.CancelledError):
        raise SystemExit(130)
    except Exception as error:
        emit("error", error_type=type(error).__name__, detail=str(error))
        raise SystemExit(1)
