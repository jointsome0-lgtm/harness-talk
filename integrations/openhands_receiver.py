"""Launch OpenHands CLI 1.16.0 / SDK 1.21.0 with an htalk receiver."""
import argparse
import asyncio
import fcntl
from importlib.metadata import version
import json
import os
from pathlib import Path
import shutil
import sys
import uuid

from managed_receiver import save


class AdmissionLedger:
    """Durable notice consumption, separate from mailbox/task completion."""
    def __init__(self, directory):
        self.directory = Path(directory).resolve()
        self.directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.lock = (self.directory / "lock").open("a")
        try:
            fcntl.flock(self.lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.path = self.directory / "state.json"
            self.state = json.loads(self.path.read_text()) if self.path.exists() else None
            if self.state is not None and (self.state.get("version") != 1
                    or self.state.get("phase") not in ("idle", "active")
                    or type(self.state.get("last_seq")) is not int
                    or self.state["last_seq"] < 0):
                raise RuntimeError("Unsupported admission state; inspect it without launching OpenHands")
        except BaseException:
            self.lock.close()
            raise

    def preflight(self):
        if self.state and (self.state["phase"] != "idle" or self.state.get("pending")):
            raise RuntimeError("OpenHands admission recovery required; inspect saved state and native history")

    def bind(self, binding):
        self.preflight()
        if self.state is not None and self.state["binding"] != binding:
            raise RuntimeError("Saved OpenHands conversation or mailbox binding differs")
        if self.state is None:
            self.state = {"version": 1, "binding": binding, "phase": "idle", "pending": None,
                          "last_seq": 0}
        self.state["phase"] = "active"
        save(self.path, self.state)

    def reserve(self, message_id, seq):
        if self.state["pending"] is not None or seq <= self.state["last_seq"]:
            raise RuntimeError("Unexpected concurrent or repeated notice admission")
        self.state["pending"] = {"id": message_id, "seq": seq}
        save(self.path, self.state)

    def consume(self, message_id, seq):
        if self.state["pending"] != {"id": message_id, "seq": seq}:
            raise RuntimeError("Uncorrelated native notice completion")
        self.state.update(last_seq=seq, pending=None)
        save(self.path, self.state)

    def finish(self):
        if self.state["pending"] is None:
            self.state["phase"] = "idle"
            save(self.path, self.state)

    def recover(self, conversation_id, message_id, disposition):
        if self.state is None or self.state["phase"] != "active":
            raise RuntimeError("No uncertain admission to recover")
        if self.state["binding"]["conversation_id"] != conversation_id:
            raise RuntimeError("Saved conversation differs; inspect status before recovering")
        pending = self.state["pending"]
        if message_id != (pending["id"] if pending else None):
            raise RuntimeError("Name the exact pending notice, or omit --message when none is pending")
        if pending is None and disposition != "consumed":
            raise RuntimeError("No pending admission; use consumed without --message")
        retired = self.directory / "recovery"
        retired.mkdir(exist_ok=True)
        save(retired / f"{uuid.uuid4()}.json", {**self.state, "disposition": disposition})
        if pending and disposition == "consumed":
            self.state["last_seq"] = pending["seq"]
        self.state.update(phase="idle", pending=None)
        save(self.path, self.state)

    def close(self):
        self.lock.close()



def build_tui(ledger):
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
        _htalk_admitting = None

        def run_worker(self, *args, **kwargs):
            worker = super().run_worker(*args, **kwargs)
            if self._htalk_admitting is not None and kwargs.get("name") == "process_message":
                self._htalk_admitting.worker = worker
            return worker

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
            self._htalk_admitting = event
            event.worker = None
            try:
                await super()._on_send_message(event)
                if event.worker is None:
                    raise RuntimeError("native admission created no identifiable worker")
                event.accepted.set_result((event.worker, self.current_runner))
            except Exception as error:
                event.accepted.set_exception(error)
            finally:
                self._htalk_admitting = None


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
                    try:
                        child.kill()
                    except ProcessLookupError:
                        pass
                    await asyncio.wait_for(child.wait(), 1)

        async def on_unmount(self):
            await self._stop_receiver()
            if ledger is not None and ledger.state is not None and not getattr(self, "_htalk_failed", False):
                manager = self.conversation_manager
                runner = manager.current_runner
                active = any(w.name in ("process_message", "resume_conversation")
                             and not w.is_finished for w in manager.workers)
                status = runner.conversation.state.execution_status.value if runner else None
                if (str(self.conversation_id) == ledger.state["binding"]["conversation_id"]
                        and runner is not None and not runner.is_running and not active
                        and status in ("idle", "finished")):
                    ledger.finish()

        async def _receive(self):
            peer, database = os.environ["HTALK_PEER"], os.environ["HTALK_DB"]
            conversation_id = self.conversation_id
            try:
                if conversation_id is None:
                    raise RuntimeError("no selected native conversation")
                ledger.bind({"conversation_id": str(conversation_id), "peer": peer,
                    "db": str(Path(database).resolve()),
                    "htalk": str(Path(shutil.which(os.environ.get("HTALK_BIN", "htalk"))
                                      or os.environ.get("HTALK_BIN", "htalk")).resolve())})
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
                    if (event.get("event") != "message" or not isinstance(event.get("notification"), str)
                            or not isinstance(event.get("id"), str) or not event["id"]
                            or type(event.get("seq")) is not int or event["seq"] <= 0):
                        raise RuntimeError("unexpected watch event")
                    if event["seq"] <= ledger.state["last_seq"]:
                        continue
                    ledger.reserve(event["id"], event["seq"])
                    # Keep one notice pending; the UI decides admission when it
                    # consumes the event, so a newly opened approval cannot race us.
                    while True:
                        if self.conversation_id != conversation_id:
                            raise RuntimeError("conversation changed")
                        notice = Notice(conversation_id, event["notification"])
                        if not self.conversation_manager.post_message(notice):
                            raise RuntimeError("conversation closed")
                        admission = await notice.accepted
                        if admission is not False:
                            native_worker, admitted_runner = admission
                            # Admission schedules work; it is not a completed turn.
                            await native_worker.wait()
                            while True:
                                if self.conversation_id != conversation_id:
                                    raise RuntimeError("conversation changed")
                                manager = self.conversation_manager
                                runner = manager.current_runner
                                if runner is not admitted_runner:
                                    raise RuntimeError("native conversation runner changed")
                                status = runner.conversation.state.execution_status.value if runner else None
                                active = any(w.name in ("process_message", "resume_conversation")
                                             and not w.is_finished for w in manager.workers)
                                if status == "finished" and not runner.is_running and not active:
                                    ledger.consume(event["id"], event["seq"])
                                    break
                                if status not in ("running", "paused", "waiting_for_confirmation", "finished"):
                                    raise RuntimeError("native notice execution did not finish successfully")
                                await asyncio.sleep(.2)
                            break
                        await asyncio.sleep(.2)
                raise RuntimeError("watch exited")
            except asyncio.CancelledError:
                raise
            except Exception:
                self._htalk_failed = True
                self.notify("htalk stopped. Check HTALK_DB, HTALK_PEER and htalk watch and saved admission state; inspect before recovery.",
                            severity="error", timeout=15)
            finally:
                await self._stop_receiver()

    return Notice, BoundManager, HtalkApp, textual_app

def main():
    os.umask(0o077)
    if len(sys.argv) > 1 and sys.argv[1] in ("htalk-status", "htalk-recover"):
        parser = argparse.ArgumentParser(description="Passive OpenHands admission inspection/recovery")
        parser.add_argument("command", choices=("htalk-status", "htalk-recover"))
        parser.add_argument("--state", required=True, type=Path)
        parser.add_argument("--conversation")
        parser.add_argument("--message")
        parser.add_argument("--disposition", choices=("retry", "consumed"))
        args = parser.parse_args()
        ledger = AdmissionLedger(args.state)
        try:
            if args.command == "htalk-status":
                print(json.dumps(ledger.state), flush=True)
            else:
                if not args.conversation or not args.disposition:
                    parser.error("Recovery requires --conversation and --disposition")
                ledger.recover(args.conversation, args.message, args.disposition)
        finally:
            ledger.close()
        return
    if not os.environ.get("HTALK_PEER") or not os.environ.get("HTALK_DB"):
        raise SystemExit("Set HTALK_PEER and HTALK_DB to the same peer and database as the MCP tool.")
    if not os.environ.get("HTALK_OPENHANDS_STATE"):
        raise SystemExit("Set HTALK_OPENHANDS_STATE to a private admission-state directory.")
    ledger = AdmissionLedger(os.environ["HTALK_OPENHANDS_STATE"])
    # Process-local factory substitution. The installed package remains untouched;
    # its normal CLI still owns arguments, settings, model and approval policy.
    try:
        ledger.preflight()  # Refuse uncertain work before importing/launching the native CLI.
        _, BoundManager, HtalkApp, textual_app = build_tui(ledger)
        textual_app.ConversationManager = BoundManager
        textual_app.OpenHandsApp = HtalkApp
        from openhands_cli.entrypoint import main as cli_main
        cli_main()
    finally:
        ledger.close()


if __name__ == "__main__":
    main()
