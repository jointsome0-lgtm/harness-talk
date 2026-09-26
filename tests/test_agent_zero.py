"""One plugin regression; framework/model loading is outside this unit fixture."""
import importlib.util
from pathlib import Path
from types import ModuleType, SimpleNamespace
import unittest
from unittest.mock import patch


class AgentZeroPauseTest(unittest.TestCase):
    def test_new_user_pause_keeps_the_wake_pending_until_idle(self):
        history, logged = [], []
        user_message = SimpleNamespace(message="owner", id="owner")
        context = SimpleNamespace(paused=False, running=False, task=None,
                                  agent0=SimpleNamespace(intervention=None))
        context.get_agent = lambda: context.agent0
        context.is_running = lambda: context.running

        def run_task(function, agent, message):
            context.running = True
            function(agent, message)
            return SimpleNamespace(message=message)

        def communicate(message, broadcast_level=1):
            # Agent Zero e3051fb's public send unconditionally clears pause.
            context.paused = False
            if context.running:
                if broadcast_level:
                    context.agent0.intervention = message
            else:
                context.task = run_task(context._process_chain, context.agent0, message)
            return context.task

        context.run_task, context.communicate = run_task, communicate
        native = ModuleType("agent")
        native.AgentContext = type("AgentContext", (), {})
        native.UserMessage = lambda message, id: SimpleNamespace(message=message, id=id)
        helpers = ModuleType("helpers")
        helpers.message_queue = SimpleNamespace(has_queue=lambda _: False,
            log_user_message=lambda *args, **kwargs: logged.append(kwargs["message_id"]))
        helpers.plugins = SimpleNamespace()
        monitor = ModuleType("helpers.state_monitor_integration")
        monitor.mark_dirty_for_context = lambda *args, **kwargs: None
        path = (Path(__file__).resolve().parents[1] / "integrations/agent-zero/"
                "htalk_notice/helpers/receiver.py")
        spec = importlib.util.spec_from_file_location("htalk_zero_test", path)
        receiver = importlib.util.module_from_spec(spec)
        with patch.dict("sys.modules", {"agent": native, "helpers": helpers,
                                        "helpers.state_monitor_integration": monitor}):
            spec.loader.exec_module(receiver)
        context.id = "selected"
        pending = receiver.Receiver(context, ("worker", context.id, "", ""))
        pending.active = lambda: True
        setattr(native.AgentContext, receiver.OWNER, pending)

        def process_chain(agent, message):
            data = {"args": (context, agent, message), "kwargs": {}}
            receiver.accept_wake(data)
            if "result" not in data:
                history.append(message)

        context._process_chain = process_chain
        pending.notice()
        new_message = receiver.UserMessage

        def user_starts_and_pauses(*args, **kwargs):
            context.communicate(user_message)
            context.paused = True
            return new_message(*args, **kwargs)

        with patch.object(receiver, "UserMessage", user_starts_and_pauses):
            pending.flush()
        self.assertTrue(context.paused)
        self.assertIs(context.task.message, user_message)
        self.assertIsNone(context.agent0.intervention)
        self.assertEqual((pending.pending, pending.accepted, logged), (1, 0, []))

        context.paused = context.running = False  # owner resumes and finishes
        pending.flush()
        wake = pending.wake
        self.assertEqual(history, [user_message, wake])
        self.assertEqual((pending.accepted, logged), (1, [wake.id]))
        context.running = False
        pending.flush()
        self.assertIs(pending.wake, wake)
        self.assertEqual(len(history), 2)


if __name__ == "__main__":
    unittest.main()
