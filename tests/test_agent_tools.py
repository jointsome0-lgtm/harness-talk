"""Hermes tool contract with captured calls and an owned stdin pipe.

Only the stdin check starts processes, both disposable Python fixtures.
No native harness, htalk mailbox, watcher, network or model is used.
"""
from contextlib import contextmanager
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch


HERMES = (Path(__file__).resolve().parents[1]
          / "integrations/hermes/htalk-notice/__init__.py")


@contextmanager
def tool_fixture():
    spec = importlib.util.spec_from_file_location("hermes_tools_fixture", HERMES)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    tools = []
    ctx = SimpleNamespace(register_tool=lambda **kwargs: tools.append(kwargs),
                          on_unload=lambda function: None)
    with patch.dict(os.environ, {"HTALK_PEER": "fixture-peer", "HTALK_BIN": "/fixture/htalk"}), \
         patch.object(module.threading.Thread, "start"), \
         patch.object(module.atexit, "register"):
        module.register(ctx)
        yield module, tools[0]


class HermesToolTest(unittest.TestCase):
    def test_mailbox_commands_preserve_arguments_identity_and_stdin_contract(self):
        cases = (["inbox", "--limit", "30"], ["show", "message-id"], ["ack", "message-id"],
                 ["sent"], ["wait", "message-id", "--seconds", "2"],
                 ["send", "other", "--message", "question"],
                 ["send", "other", "--id", "b0d4eff8-156d-4ca5-b50c-92a4a8778f57",
                  "--message", "question"],
                 ["send", "other", "--message", "--as other mcp --message-file=/not-a-file"],
                 ["reply", "message-id", "--message", "watch"],
                 ["send", "other", "--message-file", "/fixture/body.txt"],
                 ["reply", "message-id", "--message", "answer"],
                 ["peer", "list", "--all"], ["peer", "check", "other"],
                 ["--help"], ["-h"], ["--version"], ["-V"], ["send", "--help"])
        with tool_fixture() as (module, tool), \
             patch.object(module.subprocess, "run", return_value=SimpleNamespace(stdout="saved", stderr="")) as run:
            for args in cases:
                with self.subTest(args=args):
                    self.assertEqual(tool["handler"]({"args": args}), "saved")
                    call_args, kwargs = run.call_args
                    self.assertEqual(call_args[0], ["/fixture/htalk", "--as", "fixture-peer", *args])
                    self.assertIs(kwargs["stdin"], subprocess.DEVNULL)
                    self.assertEqual(kwargs["timeout"], 120)
            self.assertEqual(run.call_count, len(cases))

    def test_server_setup_and_receiver_commands_never_launch(self):
        rejected = (["mcp"], ["mcp", "--connect", "--", "other-program"],
                    ["watch"], ["receive", "--state", "/unused"], ["catalog", "list"],
                    ["migrate"], ["peer", "add", "other"], ["peer", "retire", "other"],
                    ["peer", "restore", "other"], ["peer", "discover"], ["peer"],
                    ["--as", "other", "inbox"], ["--db", "/unused", "inbox"], ["unknown"])
        with tool_fixture() as (module, tool), patch.object(module.subprocess, "run") as run:
            for args in rejected:
                with self.subTest(args=args):
                    self.assertIn("error", json.loads(tool["handler"]({"args": args})))
            run.assert_not_called()

    def test_invalid_argument_arrays_never_launch(self):
        with tool_fixture() as (module, tool), patch.object(module.subprocess, "run") as run:
            for args in (None, [], "inbox", ["inbox", 1]):
                self.assertIn("error", json.loads(tool["handler"]({"args": args})))
            run.assert_not_called()

    def test_timeout_with_or_without_id_never_automatically_retries(self):
        for id_args in ([], ["--id", "b0d4eff8-156d-4ca5-b50c-92a4a8778f57"]):
            with self.subTest(id_args=id_args), tool_fixture() as (module, tool), \
                 patch.object(module.subprocess, "run", side_effect=subprocess.TimeoutExpired("fixture", 120)) as run:
                result = json.loads(tool["handler"]({"args": ["send", "other", *id_args, "--message", "question"]}))
                self.assertIn("may already be saved", result["error"])
                self.assertIn("Inspect sent or inbox", result["next_action"])
                run.assert_called_once()

    def test_owned_parent_pipe_is_not_consumed_by_tool_child(self):
        parent_source = '''
import importlib.util, json, os, sys
from types import SimpleNamespace
from unittest.mock import patch
spec = importlib.util.spec_from_file_location("owned_hermes", sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
tools = []
ctx = SimpleNamespace(register_tool=lambda **kwargs: tools.append(kwargs), on_unload=lambda fn: None)
with patch.object(module.threading.Thread, "start"), patch.object(module.atexit, "register"):
    module.register(ctx)
    result = tools[0]["handler"]({"args": ["inbox"]})
print(json.dumps({"tool_child": json.loads(result), "parent_remaining": sys.stdin.read()}))
'''
        with tempfile.TemporaryDirectory(prefix="htalk-tool-stdin-") as directory:
            child = Path(directory) / "owned-tool-child.py"
            child.write_text("#!" + sys.executable + "\nimport json, sys\n"
                             "print(json.dumps({'stdin': sys.stdin.read(), 'argv': sys.argv[1:]}))\n")
            child.chmod(0o700)
            parent = Path(directory) / "owned-parent.py"
            parent.write_text(parent_source)
            token = "owned input must remain in the host\n"
            environment = dict(os.environ, HTALK_PEER="fixture-peer", HTALK_BIN=str(child))
            result = subprocess.run([sys.executable, "-B", str(parent), str(HERMES)],
                                    input=token, text=True, capture_output=True,
                                    timeout=10, env=environment, check=True)
            observed = json.loads(result.stdout)
            self.assertEqual(observed["tool_child"]["stdin"], "")
            self.assertEqual(observed["tool_child"]["argv"], ["--as", "fixture-peer", "inbox"])
            self.assertEqual(observed["parent_remaining"], token)


if __name__ == "__main__":
    unittest.main()
