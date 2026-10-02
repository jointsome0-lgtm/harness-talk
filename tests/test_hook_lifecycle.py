"""Adapter lifecycle regressions with modeled framework calls and POSIX locks.

No native client, model, mailbox, watcher process or receiver thread is started.
"""
from contextlib import contextmanager, redirect_stdout
import fcntl
import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import threading
from types import ModuleType, SimpleNamespace
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]


def load(name, relative):
    spec = importlib.util.spec_from_file_location(name, ROOT / relative)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@contextmanager
def zero_fixture():
    contexts, scheduled, logged, dirty, errors = {}, [], [], [], []
    native = ModuleType("agent")
    native.AgentContext = type("AgentContext", (), {"get": staticmethod(contexts.get),
                                                   "_contexts_lock": threading.RLock()})
    native.UserMessage = lambda message, id: SimpleNamespace(message=message, id=id)
    helpers = ModuleType("helpers")
    helpers.plugins = SimpleNamespace(get_enabled_plugins=lambda _: ["htalk_notice"])
    helpers.message_queue = SimpleNamespace(has_queue=lambda _: False,
        log_user_message=lambda *args, **kwargs: logged.append(kwargs["message_id"]))
    monitor = ModuleType("helpers.state_monitor_integration")
    monitor.mark_dirty_for_context = lambda *args, **kwargs: dirty.append(kwargs["reason"])
    context = SimpleNamespace(id="selected", paused=False, agent0=SimpleNamespace(),
        task=None, log=SimpleNamespace(log=lambda **kwargs: errors.append(kwargs["content"])))
    context.get_agent = lambda: context.agent0
    context.is_running = lambda: False
    context._process_chain = lambda: None
    context.run_task = lambda function, agent, message: scheduled.append(message)
    contexts[context.id] = context
    environment = {"HTALK_PEER": "worker", "HTALK_CONTEXT": context.id,
                   "HTALK_BIN": "", "HTALK_DB": ""}
    with patch.dict(sys.modules, {"agent": native, "helpers": helpers,
        "helpers.state_monitor_integration": monitor}), patch.dict(os.environ, environment):
        module = load("zero_lifecycle", "integrations/agent-zero/htalk_notice/helpers/receiver.py")
        receiver = module.Receiver(context, module.settings())
        setattr(native.AgentContext, module.OWNER, receiver)
        yield SimpleNamespace(module=module, native=native, context=context,
            receiver=receiver, scheduled=scheduled, logged=logged, dirty=dirty,
            errors=errors, helpers=helpers, monitor=monitor)


def dispatch(fixture, message, module=None):
    data = {"args": (fixture.context, fixture.context.agent0, message)}
    (module or fixture.module).accept_wake(data)
    return data


class AgentZeroLifecycleTest(unittest.TestCase):
    def test_replaced_owner_rejects_delayed_wake_and_preserves_owner_input(self):
        with zero_fixture() as f:
            f.receiver.notice()
            f.receiver.flush()
            old_wake = f.scheduled[0]
            replacement = f.module.Receiver(f.context, f.module.settings())
            setattr(f.native.AgentContext, f.module.OWNER, replacement)
            f.receiver.stop()
            self.assertIn("result", dispatch(f, old_wake))
            owner = f.module.UserMessage(old_wake.message, old_wake.id)
            self.assertNotIn("result", dispatch(f, owner))
            replacement.notice()
            replacement.flush()
            self.assertNotIn("result", dispatch(f, f.scheduled[-1]))
            self.assertEqual((f.receiver.accepted, replacement.accepted), (0, 1))
            self.assertEqual(f.logged, [replacement.wake.id])

    def test_module_reload_keeps_delayed_wake_origin(self):
        with zero_fixture() as f:
            f.receiver.notice()
            f.receiver.flush()
            reloaded = load("zero_lifecycle_reload", "integrations/agent-zero/htalk_notice/helpers/receiver.py")
            self.assertNotIn("result", dispatch(f, f.receiver.wake, reloaded))
            self.assertEqual(f.receiver.accepted, 1)
            replacement = reloaded.Receiver(f.context, reloaded.settings())
            setattr(f.native.AgentContext, reloaded.OWNER, replacement)
            self.assertIn("result", dispatch(f, f.receiver.wake, reloaded))
            self.assertEqual(len(f.logged), 1)

    def test_stale_flush_cannot_schedule_after_owner_replacement(self):
        with zero_fixture() as f:
            f.receiver.notice()
            setattr(f.native.AgentContext, f.module.OWNER, None)
            f.receiver.flush()
            self.assertEqual(f.scheduled, [])

    def test_module_reload_does_not_restart_failed_receiver_or_replay_partial_log(self):
        with zero_fixture() as f:
            def partial_log(*args, **kwargs):
                f.logged.append(kwargs["message_id"])
                raise RuntimeError("synthetic partial log")
            f.helpers.message_queue.log_user_message = partial_log
            f.receiver.notice()
            f.receiver.flush()
            with self.assertLogs(f.module.log, level="ERROR"):
                dispatch(f, f.receiver.wake)
            reloaded = load("zero_lifecycle_reload_failed", "integrations/agent-zero/htalk_notice/helpers/receiver.py")
            with patch.object(reloaded.Receiver, "start") as start:
                reloaded.reconcile()
                start.assert_not_called()
            self.assertIs(getattr(f.native.AgentContext, f.module.OWNER), f.receiver)
            self.assertEqual((f.receiver.pending, f.receiver.accepted), (1, 0))
            self.assertEqual(f.logged, [f.receiver.wake.id])

    def test_logging_failures_stop_without_replay_or_consuming_generation(self):
        for stage in ("before_log", "partial_log", "dirty", "error_log"):
            with self.subTest(stage=stage), zero_fixture() as f:
                def broken_log(*args, **kwargs):
                    if stage == "partial_log":
                        f.logged.append(kwargs["message_id"])
                    raise RuntimeError("synthetic log failure")

                def broken_dirty(*args, **kwargs):
                    raise RuntimeError("synthetic dirty failure")

                def broken_error_log(**kwargs):
                    raise RuntimeError("synthetic UI failure")

                if stage == "dirty":
                    f.module.mark_dirty_for_context = broken_dirty
                else:
                    f.helpers.message_queue.log_user_message = broken_log
                if stage == "error_log":
                    f.context.log.log = broken_error_log
                f.receiver.notice()
                f.receiver.flush()
                with self.assertLogs(f.module.log, level="ERROR") as fallback:
                    self.assertIn("result", dispatch(f, f.receiver.wake))
                self.assertIn("will not be retried automatically", fallback.output[0])
                self.assertTrue(f.receiver.failed)
                self.assertTrue(f.receiver.stopped.is_set())
                self.assertEqual((f.receiver.pending, f.receiver.accepted), (1, 0))
                f.receiver.notice()
                f.receiver.flush()
                self.assertEqual(len(f.scheduled), 1)
                self.assertIn("result", dispatch(f, f.receiver.wake))
                self.assertEqual(len(f.logged), int(stage in ("partial_log", "dirty")))
                self.assertEqual(f.dirty, [])
                self.assertNotIn("result", dispatch(f, f.module.UserMessage("owner", "owner")))


class CopilotLifecycleTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="htalk-hook-test-")
        self.addCleanup(self.directory.cleanup)
        self.environment = patch.dict(os.environ, {"HTALK_PEER": "worker",
            "HTALK_DB": str(Path(self.directory.name) / "unused.sqlite3"),
            "HTALK_COPILOT_STATE": self.directory.name})
        self.environment.start()
        self.addCleanup(self.environment.stop)
        self.module = load("copilot_lifecycle", "integrations/copilot.py")
        self.hook("start")

    def hook(self, kind, completion="shell_completed"):
        output = io.StringIO()
        event = {"sessionId": "selected", "notification_type": completion}
        with patch.object(sys, "stdin", io.StringIO(json.dumps(event))), redirect_stdout(output):
            self.module.hook(kind)
        return json.loads(output.getvalue())

    def read_state(self):
        return json.loads((Path(self.directory.name) / "state.json").read_text())

    def test_both_completion_types_preserve_pending_until_waiter_unlocks(self):
        for completion in ("shell_completed", "shell_detached_completed"):
            with self.subTest(completion=completion):
                with self.module.state() as saved:
                    saved.update(pending="saved notice", offered=["message-id"])
                with (Path(self.directory.name) / "wait.lock").open("a") as lock:
                    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    self.assertEqual(self.hook("notice", completion), {})
                    self.assertEqual(self.read_state()["pending"], "saved notice")
                notice = self.hook("notice", completion)
                self.assertIn("saved notice", notice["additionalContext"])
                self.assertIn("mode async", notice["additionalContext"])
                self.assertIsNone(self.read_state()["pending"])
                self.assertEqual(self.read_state()["offered"], ["message-id"])
                self.assertEqual(self.hook("notice", completion), {})

    def test_original_wait_path_defers_unrelated_completion_during_child_cleanup(self):
        notice = {"event": "message", "id": "message-id", "notification": "saved notice"}
        output = io.BytesIO((json.dumps(notice) + "\n").encode())
        cleanup_hooks, launches = [], []

        def wait(timeout=None):
            cleanup_hooks.append(self.hook("notice"))
            self.assertEqual(self.read_state()["pending"], "saved notice")

        child = SimpleNamespace(stdout=output, poll=lambda: None, terminate=lambda: None, wait=wait)
        selector = SimpleNamespace(register=lambda *args: None, select=lambda *args: [True])
        selector_type = type("Selector", (), {"__enter__": lambda _: selector,
                                             "__exit__": lambda *args: None})
        def launch(*args, **kwargs):
            launches.append(args[0])
            return child

        with patch.object(self.module.subprocess, "Popen", launch), \
             patch.object(self.module.selectors, "DefaultSelector", selector_type), \
             redirect_stdout(io.StringIO()):
            self.module.wait()
        self.assertEqual(cleanup_hooks, [{}])
        self.assertEqual(len(launches), 1)
        self.assertIn("saved notice", self.hook("notice")["additionalContext"])
        self.assertFalse(Path(os.environ["HTALK_DB"]).exists())

    def test_non_completion_and_closed_session_do_not_consume_pending(self):
        with self.module.state() as saved:
            saved["pending"] = "saved notice"
        self.assertEqual(self.hook("notice", "agent_completed"), {})
        self.hook("end")
        self.assertEqual(self.hook("notice"), {})
        self.assertEqual(self.read_state()["pending"], "saved notice")


if __name__ == "__main__":
    unittest.main()
