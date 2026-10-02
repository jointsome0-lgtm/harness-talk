"""Owned Linux process groups; no native client, mailbox or installed receiver."""
import asyncio
import ctypes
import importlib.util
import json
import os
from pathlib import Path
import signal
import sys
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest.mock import AsyncMock, Mock, patch


SOURCE = Path(__file__).resolve().parents[1] / "integrations/managed_receiver.py"
spec = importlib.util.spec_from_file_location("managed_receiver_under_test", SOURCE)
receiver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(receiver)

FIXTURE = """
import ctypes, json, os, signal, sys, time
from pathlib import Path
directory, case = Path(sys.argv[1]), sys.argv[2]
signal.signal(signal.SIGALRM, lambda *_: os._exit(91))
signal.alarm(8)
if case != 'single':
    child = os.fork()
    if child == 0:
        signal.alarm(8)
        signal.signal(signal.SIGTERM, signal.SIG_DFL if case == 'cooperative' else signal.SIG_IGN)
        if case == 'nondumpable' and ctypes.CDLL(None).prctl(4, 0, 0, 0, 0):
            os._exit(92)
        (directory / 'descendant.json').write_text(json.dumps({'pid': os.getpid()}))
        while True:
            time.sleep(0.01)
def stop(*_):
    if case == 'late' and os.fork() == 0:
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        signal.alarm(8)
        (directory / 'late.json').write_text(json.dumps({'pid': os.getpid()}))
        while True:
            time.sleep(0.01)
    sys.exit(0)
signal.signal(signal.SIGTERM, stop)
(directory / 'leader.json').write_text(json.dumps({'pid': os.getpid()}))
while True:
    time.sleep(0.01)
"""


class ManagedReceiverDisposal(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.libc = ctypes.CDLL(None, use_errno=True)
        self.old_subreaper = ctypes.c_int()
        for option, argument in ((37, ctypes.byref(self.old_subreaper)), (36, 1)):
            if self.libc.prctl(option, argument, 0, 0, 0):
                raise OSError(ctypes.get_errno(), "fixture subreaper setup failed")
        self.temporary = tempfile.TemporaryDirectory(prefix="htalk-dispose-test-")
        self.directory = Path(self.temporary.name)
        self.process = None
        self.identities = {}
        self.events = []
        self.patches = [patch.object(receiver, "SHUTDOWN_GRACE_SECONDS", 0.1),
                        patch.object(receiver, "SHUTDOWN_KILL_SECONDS", 0.5),
                        patch.object(receiver, "emit", self.capture_event)]
        for item in self.patches:
            item.start()

    def capture_event(self, event, **fields):
        self.events.append({"event": event, **fields,
                            "live_pids": self.live_pids()})

    def live_pids(self):
        return [pid for pid, identity in self.identities.items()
                if (current := receiver._process_identity(pid))
                and current[:3] == identity[:3] and current[3] not in ("Z", "X")]

    async def launch(self, case):
        self.process = await asyncio.create_subprocess_exec(
            sys.executable, "-B", "-c", FIXTURE, str(self.directory), case,
            stdin=asyncio.subprocess.DEVNULL, stdout=asyncio.subprocess.DEVNULL,
            stderr=asyncio.subprocess.DEVNULL, start_new_session=True)
        identity = receiver._process_identity(self.process.pid)
        self.identities[self.process.pid] = identity
        self.process._htalk_group_identity = identity
        deadline = time.monotonic() + 2
        roles = ("leader",) if case == "single" else ("leader", "descendant")
        for role in roles:
            path = self.directory / f"{role}.json"
            while True:
                try:
                    pid = json.loads(path.read_text())["pid"]
                    break
                except (FileNotFoundError, json.JSONDecodeError):
                    if time.monotonic() >= deadline:
                        self.fail("fixture readiness deadline exceeded")
                    await asyncio.sleep(0.01)
            identity = receiver._process_identity(pid)
            self.assertIsNotNone(identity)
            self.assertEqual(identity[:2], (self.process.pid, self.process.pid))
            self.identities[pid] = identity

    async def asyncTearDown(self):
        try:
            # A failed readiness check may still have produced a late child record.
            for path in self.directory.glob("*.json"):
                try:
                    pid = json.loads(path.read_text())["pid"]
                    identity = receiver._process_identity(pid)
                    if identity and identity[:2] == (self.process.pid, self.process.pid):
                        self.identities.setdefault(pid, identity)
                except (FileNotFoundError, json.JSONDecodeError):
                    pass
            for pid, identity in self.identities.items():
                current = receiver._process_identity(pid)
                if current and current[:3] == identity[:3] and current[3] not in ("Z", "X"):
                    fd = os.pidfd_open(pid)
                    try:
                        if (current := receiver._process_identity(pid)) and current[:3] == identity[:3]:
                            signal.pidfd_send_signal(fd, signal.SIGKILL)
                    finally:
                        os.close(fd)
            if self.process is not None:
                await asyncio.wait_for(self.process.wait(), 2)
            deadline = time.monotonic() + 2
            while any(receiver._process_identity(pid) for pid in self.identities):
                for pid in self.identities:
                    current = receiver._process_identity(pid)
                    if current and current[4] == os.getpid() and pid != self.process.pid:
                        try:
                            os.waitpid(pid, os.WNOHANG)
                        except ChildProcessError:
                            pass
                if time.monotonic() >= deadline:
                    self.fail(f"fixture PID cleanup deadline exceeded: {self.identities}")
                await asyncio.sleep(0.01)
            self.assertTrue(all(not Path(f"/proc/{pid}").exists() for pid in self.identities))
        finally:
            for item in reversed(self.patches):
                item.stop()
            self.temporary.cleanup()
            if self.libc.prctl(36, self.old_subreaper.value, 0, 0, 0):
                raise OSError(ctypes.get_errno(), "fixture subreaper restore failed")

    async def test_leader_exit_does_not_skip_stubborn_descendant_escalation(self):
        await self.launch("stubborn")
        started = time.monotonic()
        await asyncio.wait_for(receiver.dispose(self.process), 2)
        elapsed = time.monotonic() - started
        self.assertGreaterEqual(elapsed, 0.1)
        self.assertLess(elapsed, 1)
        self.assertEqual(self.process.returncode, 0)
        self.assertEqual(len(self.events), 1)
        self.assertEqual(self.events[0]["event"], "child_stopped")
        self.assertEqual(self.events[0]["live_pids"], [])
        await asyncio.sleep(0.2)
        self.assertEqual(self.live_pids(), [])

    async def test_cooperative_group_finishes_without_sigkill(self):
        await self.launch("cooperative")
        real_signal = signal.pidfd_send_signal
        with patch.object(signal, "pidfd_send_signal", wraps=real_signal) as send:
            await asyncio.wait_for(receiver.dispose(self.process), 2)
        self.assertFalse(any(call.args[1] == signal.SIGKILL for call in send.call_args_list))
        self.assertEqual(self.events[0]["live_pids"], [])

    async def test_nondumpable_descendant_is_not_omitted_when_proc_inode_is_root_owned(self):
        await self.launch("nondumpable")
        pid = next(pid for pid in self.identities if pid != self.process.pid)
        directory = Path(f"/proc/{pid}")
        real_stat = Path.stat

        def root_owned(path, *args, **kwargs):
            result = real_stat(path, *args, **kwargs)
            # User namespaces can map procfs root ownership to our UID. Keep the
            # real non-dumpable child and exercise the host's root-inode branch.
            return SimpleNamespace(st_uid=0) if path == directory else result

        with patch.object(Path, "stat", root_owned):
            await asyncio.wait_for(receiver.dispose(self.process), 2)
        self.assertEqual(self.events[0]["live_pids"], [])
        self.assertEqual(self.live_pids(), [])

    async def test_new_descendant_is_enrolled_while_original_descendant_anchors_group(self):
        await self.launch("late")
        await asyncio.wait_for(receiver.dispose(self.process), 2)
        pid = json.loads((self.directory / "late.json").read_text())["pid"]
        identity = receiver._process_identity(pid)
        self.assertIsNotNone(identity)
        self.identities[pid] = identity
        self.assertEqual(receiver._group_members(self.process.pid), {})
        self.assertEqual(self.live_pids(), [])

    async def test_signal_failure_propagates_without_success_event(self):
        await self.launch("stubborn")
        with patch.object(signal, "pidfd_send_signal", side_effect=PermissionError("fixture refusal")):
            with self.assertRaises(PermissionError):
                await receiver.dispose(self.process)
        self.assertEqual(self.events, [])

    async def test_identity_read_failure_closes_opened_pidfd(self):
        await self.launch("single")
        opened = []
        real_open = os.pidfd_open

        def pin(pid):
            fd = real_open(pid)
            opened.append(fd)
            return fd

        with patch.object(os, "pidfd_open", side_effect=pin), patch.object(
                receiver, "_process_identity", side_effect=[
                    self.identities[self.process.pid], PermissionError("fixture stat refusal")]):
            with self.assertRaises(PermissionError):
                await receiver.dispose(self.process)
        self.assertEqual(len(opened), 1)
        with self.assertRaises(OSError):
            os.fstat(opened[0])
        self.assertEqual(self.events, [])

    async def test_none_has_no_event_or_signal(self):
        with patch.object(signal, "pidfd_send_signal") as send:
            await receiver.dispose(None)
        send.assert_not_called()
        self.assertEqual(self.events, [])

    async def test_already_exited_empty_group_is_waited_without_signalling(self):
        await self.launch("single")
        os.kill(self.process.pid, signal.SIGTERM)
        await asyncio.wait_for(self.process.wait(), 2)
        with patch.object(signal, "pidfd_send_signal") as send:
            await receiver.dispose(self.process)
        send.assert_not_called()
        self.assertEqual(self.events[0]["live_pids"], [])

    async def test_reaped_leader_with_unproven_group_fails_without_signal_or_event(self):
        await self.launch("stubborn")
        os.kill(self.process.pid, signal.SIGTERM)
        await asyncio.wait_for(self.process.wait(), 2)
        with patch.object(signal, "pidfd_send_signal") as send:
            with self.assertRaisesRegex(RuntimeError, "Cannot prove.*ownership"):
                await receiver.dispose(self.process)
        send.assert_not_called()
        self.assertEqual(self.events, [])
        self.assertEqual(len(self.live_pids()), 1)

    async def test_mismatched_leader_birth_is_not_treated_as_owned(self):
        await self.launch("stubborn")
        identity = self.process._htalk_group_identity
        self.process._htalk_group_identity = (*identity[:2], identity[2] - 1, *identity[3:])
        with patch.object(signal, "pidfd_send_signal") as send:
            with self.assertRaisesRegex(RuntimeError, "Cannot prove.*ownership"):
                await receiver.dispose(self.process)
        send.assert_not_called()
        self.assertEqual(self.events, [])

    async def test_exit_during_signal_is_a_normal_cleanup_race(self):
        await self.launch("single")
        real_signal = signal.pidfd_send_signal

        def exits_first(fd, sig):
            real_signal(fd, signal.SIGKILL)
            raise ProcessLookupError("fixture exited immediately before signal")

        with patch.object(signal, "pidfd_send_signal", side_effect=exits_first):
            await asyncio.wait_for(receiver.dispose(self.process), 2)
        self.assertEqual(self.events[0]["live_pids"], [])

    async def test_cancellation_waits_for_cleanup_then_propagates_without_success_event(self):
        await self.launch("stubborn")
        term_sent = asyncio.Event()
        real_signal = signal.pidfd_send_signal

        def send(fd, sig):
            real_signal(fd, sig)
            if sig == signal.SIGTERM:
                term_sent.set()

        with patch.object(signal, "pidfd_send_signal", side_effect=send):
            task = asyncio.create_task(receiver.dispose(self.process))
            await asyncio.wait_for(term_sent.wait(), 1)
            task.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await asyncio.wait_for(task, 2)
        self.assertIsNotNone(self.process.returncode)
        self.assertEqual(self.live_pids(), [])
        self.assertEqual(self.events, [])


class ManagedWorkerCleanup(unittest.IsolatedAsyncioTestCase):
    async def test_unsupported_pidfds_fail_before_preparing_or_starting_native_session(self):
        with tempfile.TemporaryDirectory(prefix="htalk-worker-preflight-test-") as directory:
            state = {"phase": "new", "binding": {}}
            args = SimpleNamespace(state=Path(directory))
            adapter = SimpleNamespace(prepare=Mock(), Session=Mock())
            with patch.object(os, "pidfd_open", side_effect=OSError("pidfds unavailable")):
                with self.assertRaises(OSError):
                    await receiver.worker(args, state, adapter)
            adapter.prepare.assert_not_called()
            adapter.Session.assert_not_called()
            self.assertEqual(state["phase"], "new")
            self.assertFalse((args.state / receiver.STATE_FILE).exists())

    async def check_cleanup_failure(self, error, expected_phase):
        with tempfile.TemporaryDirectory(prefix="htalk-worker-cleanup-test-") as directory:
            state = {"phase": "new", "binding": {"htalk": "fixture", "db": "fixture", "peer": "fixture"},
                     "session_id": "fixture", "pending": None, "last_seq": 0}
            args = SimpleNamespace(state=Path(directory))
            reader = asyncio.get_running_loop().create_future()
            session = SimpleNamespace(start=AsyncMock(), reader=reader, close=AsyncMock())
            adapter = SimpleNamespace(prepare=Mock(), Session=Mock(return_value=session))
            output = asyncio.StreamReader()
            output.feed_eof()
            watch = SimpleNamespace(pid=1, stdout=output)
            try:
                with patch.object(asyncio, "create_subprocess_exec", AsyncMock(return_value=watch)), \
                        patch.object(receiver, "_process_identity", return_value=None), \
                        patch.object(receiver, "emit"), \
                        patch.object(receiver, "dispose", AsyncMock(side_effect=error)) as dispose:
                    with self.assertRaises(type(error)):
                        await receiver.worker(args, state, adapter)
                dispose.assert_awaited_once_with(watch)
                session.close.assert_awaited_once()
                self.assertEqual(json.loads((args.state / receiver.STATE_FILE).read_text())["phase"],
                                 expected_phase)
            finally:
                reader.cancel()

    async def test_disposer_error_closes_native_session_and_requires_inspection(self):
        await self.check_cleanup_failure(RuntimeError("fixture cleanup failure"), "needs_inspection")

    async def test_disposer_cancellation_still_closes_native_session(self):
        await self.check_cleanup_failure(asyncio.CancelledError(), "idle")


if __name__ == "__main__":
    unittest.main()
