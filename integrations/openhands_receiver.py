"""Launch OpenHands CLI 1.16.0 / SDK 1.21.0 with an htalk receiver."""
import asyncio
from importlib.metadata import version
import json
import os
from pathlib import Path

if version("openhands") != "1.16.0" or version("openhands-sdk") != "1.21.0":
    raise SystemExit("This htalk launcher requires OpenHands CLI 1.16.0 / SDK 1.21.0; check the TUI interface before upgrading.")

from textual import on

from openhands_cli.tui import textual_app
from openhands_cli.tui.core.conversation_manager import ConversationManager
from openhands_cli.tui.messages import SendMessage


class Notice(SendMessage):
    def __init__(self, conversation_id, text):
        super().__init__(text)
        self.conversation_id = conversation_id
        self.accepted = asyncio.get_running_loop().create_future()


class BoundManager(ConversationManager):
    @on(Notice)
    async def _on_notice(self, event):
        # Textual also dispatches base-class handlers unless prevented.
        event.prevent_default()
        event.stop()
        # Check at consumption, not when posting: /new may already be queued.
        if event.conversation_id != self.state.conversation_id:
            event.accepted.set_exception(RuntimeError("conversation changed"))
            return
        runner = self.current_runner
        active_worker = any(w.name in ("process_message", "resume_conversation")
                            and not w.is_finished for w in self.workers)
        status = runner.conversation.state.execution_status.value if runner else None
        if active_worker or (runner and runner.is_running) or status in (
                "paused", "waiting_for_confirmation"):
            # A native run() can approve pending actions. Wait for the human and
            # the whole native worker before entering its ordinary input path.
            event.accepted.set_result(False)
            return
        await super()._on_send_message(event)
        event.accepted.set_result(True)


class HtalkApp(textual_app.OpenHandsApp):
    # Textual resolves relative CSS against the subclass's file.
    CSS_PATH = str(Path(textual_app.__file__).with_suffix(".tcss"))
    _htalk_process = None
    _htalk_started = False

    def _initialize_main_ui(self):
        super()._initialize_main_ui()
        if not self._htalk_started and not self.headless_mode:
            self._htalk_started = True
            self.run_worker(self._receive(), name="htalk", group="htalk", exit_on_error=False)

    async def _stop_receiver(self):
        child = self._htalk_process
        if child and child.returncode is None:
            try:
                child.terminate()
            except ProcessLookupError:
                pass
            try:
                await asyncio.wait_for(child.wait(), 3)
            except asyncio.TimeoutError:
                child.kill()
                await child.wait()

    async def on_unmount(self):
        await self._stop_receiver()

    async def _receive(self):
        peer, database = os.environ["HTALK_PEER"], os.environ["HTALK_DB"]
        conversation_id = self.conversation_id
        try:
            child = await asyncio.create_subprocess_exec(
                os.environ.get("HTALK_BIN", "htalk"), "--db", database, "--as", peer, "watch",
                stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.DEVNULL,
            )
            self._htalk_process = child
            while line := await child.stdout.readline():
                if self.conversation_id != conversation_id:
                    raise RuntimeError("conversation changed")
                event = json.loads(line)
                if event.get("event") == "ready" and event.get("peer") == peer:
                    self.notify(f"htalk listening as {peer}")
                    continue
                if event.get("event") != "message" or not isinstance(event.get("notification"), str):
                    raise RuntimeError("unexpected watch event")
                # Keep one notice pending; the UI decides admission when it
                # consumes the event, so a newly opened approval cannot race us.
                while True:
                    if self.conversation_id != conversation_id:
                        raise RuntimeError("conversation changed")
                    notice = Notice(conversation_id, event["notification"])
                    if not self.conversation_manager.post_message(notice):
                        raise RuntimeError("conversation closed")
                    if await notice.accepted:
                        break
                    await asyncio.sleep(.2)
            raise RuntimeError("watch exited")
        except asyncio.CancelledError:
            raise
        except Exception:
            self.notify("htalk stopped. Check HTALK_DB, HTALK_PEER and htalk watch; restart to reconnect.",
                        severity="error", timeout=15)
        finally:
            await self._stop_receiver()


def main():
    if not os.environ.get("HTALK_PEER") or not os.environ.get("HTALK_DB"):
        raise SystemExit("Set HTALK_PEER and HTALK_DB to the same peer and database as the MCP tool.")
    # Process-local factory substitution. The installed package remains untouched;
    # its normal CLI still owns arguments, settings, model and approval policy.
    textual_app.ConversationManager = BoundManager
    textual_app.OpenHandsApp = HtalkApp
    from openhands_cli.entrypoint import main as cli_main
    cli_main()


if __name__ == "__main__":
    main()
