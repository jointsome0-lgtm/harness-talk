"""Synthetic adapter contracts. No installed SDK, provider, native client or mailbox."""
import asyncio
from contextlib import ExitStack
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
from types import ModuleType, SimpleNamespace as NS
import unittest
from unittest.mock import AsyncMock, Mock, patch

ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, ROOT / path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


managed = load('managed_receiver', 'integrations/managed_receiver.py')
with patch.dict(sys.modules, {'managed_receiver': managed}):
    agy = load('agy_fixture', 'integrations/antigravity.py')
    letta = load('letta_fixture', 'integrations/letta.py')
    oh = load('openhands_fixture', 'integrations/openhands_receiver.py')


def initial_state(phase='new', lifecycle=True):
    state = {'version': 1, 'phase': phase, 'binding': {'htalk': '/fixture/htalk',
             'db': '/fixture/unused.db', 'peer': 'receiver'}, 'session_id': 'session',
             'pending': None, 'last_seq': 0}
    if lifecycle:
        state['lifecycle_version'] = 1
    return state


class ManagedLifecycle(unittest.IsolatedAsyncioTestCase):
    async def run_fixture(self, directory, state, close, *, failure=None):
        reader = asyncio.get_running_loop().create_future()
        output = asyncio.StreamReader()
        output.feed_data(b'{"event":"message","id":"notice","seq":1,"notification":"synthetic"}\n')
        session = NS(start=AsyncMock(side_effect=failure), prompt=AsyncMock(return_value='end_turn'),
                     close=close, reader=reader)
        adapter = NS(CLEAN_SHUTDOWN_REQUIRED=agy.CLEAN_SHUTDOWN_REQUIRED,
                     prepare=Mock(), Session=Mock(return_value=session))
        args = NS(state=Path(directory), task=None, max_turns=1)
        watch = NS(pid=os.getpid(), stdout=output)
        stack = ExitStack()
        stack.enter_context(patch.object(asyncio, 'create_subprocess_exec', AsyncMock(return_value=watch)))
        stack.enter_context(patch.object(managed, '_group_members', return_value={}))
        stack.enter_context(patch.object(managed, '_process_identity', return_value=None))
        stack.enter_context(patch.object(managed, 'dispose', AsyncMock()))
        stack.enter_context(patch.object(managed, 'emit'))
        stack.enter_context(patch.object(managed, 'mail', AsyncMock(side_effect=[
            {'recipient': 'receiver', 'ack_at': None, 'in_reply_to': None, 'reply': None},
            {'recipient': 'receiver', 'ack_at': 1, 'in_reply_to': None, 'reply': None}])))
        self.addCleanup(stack.close)
        self.addCleanup(reader.cancel)
        return args, adapter, session, asyncio.create_task(managed.worker(args, state, adapter))

    async def test_result_and_ack_do_not_release_active_runtime_before_clean_close(self):
        with tempfile.TemporaryDirectory() as directory:
            entered, release = asyncio.Event(), asyncio.Event()
            async def close():
                entered.set()
                await release.wait()
            state = initial_state()
            args, adapter, session, task = await self.run_fixture(directory, state, close)
            await asyncio.wait_for(entered.wait(), 1)
            saved = managed.read_state(Path(directory))
            self.assertEqual((saved['phase'], saved['last_seq'], saved['pending']), ('active', 1, None))
            # Model loss of both lock holders: the saved active phase itself must
            # refuse preparation/resume, without asserting native survival.
            with patch.object(adapter, 'prepare') as prepare:
                self.assertEqual(await managed.worker(args, dict(saved), adapter), 3)
                prepare.assert_not_called()
            release.set()
            self.assertEqual(await task, 0)
            self.assertEqual(managed.read_state(Path(directory))['phase'], 'idle')
            # Positive clean-close restart resumes, with the same saved ID.
            adapter.Session.return_value = NS(start=AsyncMock(side_effect=RuntimeError('resumed fixture')),
                close=AsyncMock(), reader=session.reader)
            with self.assertRaisesRegex(RuntimeError, 'resumed fixture'):
                await managed.worker(args, state, adapter)
            adapter.Session.return_value.start.assert_awaited_once_with(False)

    async def test_actual_letta_clean_close_waits_eof_cleanup_before_resumable_idle(self):
        self.assertTrue(letta.CLEAN_SHUTDOWN_REQUIRED)
        with tempfile.TemporaryDirectory() as directory:
            state = initial_state()
            args = NS(state=Path(directory))
            native = letta.Session(args, {**state, 'binding': {**state['binding'],
                'letta': '/fixture/letta', 'agent': 'agent-local-fixture'}})
            entered, release = asyncio.Event(), asyncio.Event()
            native.reader = asyncio.get_running_loop().create_future()
            async def wait():
                entered.set()
                await release.wait()
                native.process.returncode = 0
                native.reader.set_result(None)
                return 0
            native.process = NS(stdin=NS(close=Mock()), returncode=None, wait=wait)
            with patch.object(letta, 'dispose', AsyncMock()) as dispose:
                args, adapter, _, task = await self.run_fixture(directory, state, native.close)
                adapter.CLEAN_SHUTDOWN_REQUIRED = letta.CLEAN_SHUTDOWN_REQUIRED
                await asyncio.wait_for(entered.wait(), 1)
                self.assertEqual(managed.read_state(Path(directory))['phase'], 'active')
                native.process.stdin.close.assert_called_once()
                release.set()
                self.assertEqual(await task, 0)
                dispose.assert_awaited_once_with(native.process)
                self.assertEqual(managed.read_state(Path(directory))['phase'], 'idle')

    async def test_close_error_or_cancellation_never_persists_safe_idle(self):
        for error in [RuntimeError('sync failed'), asyncio.CancelledError()]:
            with self.subTest(error=type(error).__name__), tempfile.TemporaryDirectory() as directory:
                state = initial_state()
                close = AsyncMock(side_effect=error)
                _, _, _, task = await self.run_fixture(directory, state, close)
                with self.assertRaises(type(error)):
                    await task
                self.assertEqual(managed.read_state(Path(directory))['phase'], 'needs_inspection')
                self.assertEqual(state['last_seq'], 1)

    async def test_start_and_prompt_failures_do_not_release_lease(self):
        for where in ['start', 'prompt']:
            with self.subTest(where=where), tempfile.TemporaryDirectory() as directory:
                state = initial_state()
                args, adapter, session, task = await self.run_fixture(directory, state, AsyncMock(),
                    failure=RuntimeError('start failed') if where == 'start' else None)
                if where == 'prompt':
                    session.prompt.side_effect = RuntimeError('prompt failed')
                with self.assertRaises(RuntimeError):
                    await task
                self.assertNotEqual(managed.read_state(Path(directory))['phase'], 'idle')
                session.close.assert_awaited_once()

    async def test_legacy_idle_refuses_native_launch_and_explicit_recovery_preserves_receipt(self):
        with tempfile.TemporaryDirectory() as directory:
            state = initial_state('idle', lifecycle=False)
            args = NS(state=Path(directory), discard_session='session', message=None, disposition='settled')
            adapter = NS(CLEAN_SHUTDOWN_REQUIRED=True, prepare=Mock(), Session=Mock(), RECOVERY_NOTE='retained')
            with patch.object(managed, 'emit'):
                self.assertEqual(await managed.worker(args, state, adapter), 3)
                self.assertEqual(state['session_id'], 'session')
                adapter.prepare.assert_not_called()
                adapter.Session.assert_not_called()
                self.assertEqual(await managed.recover(args, state, adapter), 0)
            receipt = json.loads(next((Path(directory) / 'retired').glob('*.json')).read_text())
            self.assertEqual(receipt['session_id'], 'session')
            self.assertEqual(receipt['binding'], state['binding'])
            self.assertEqual(receipt['phase'], 'needs_inspection')
            self.assertEqual((state['phase'], state['session_id']), ('new', None))


class AntigravityProtocol(unittest.IsolatedAsyncioTestCase):
    async def test_configured_and_correlated_complete_result(self):
        args = NS(state=Path('/fixture'))
        state = {'pending': {'id': 'notice'}, 'session_id': 'session'}
        session = agy.Session(args, state)
        session.initial = asyncio.get_running_loop().create_future()
        session.turn = asyncio.get_running_loop().create_future()
        output = asyncio.StreamReader()
        output.feed_data(b'{"event":"configured","configured_session_id":"session"}\n')
        output.feed_data(b'{"event":"result","message_id":"notice","session_id":"session","complete":true}\n')
        output.feed_eof()
        session.process = NS(stdout=output)
        session.closing = True
        with patch.object(agy, 'emit'):
            await session.read()
        self.assertIsNone(session.failure)
        self.assertEqual(await session.turn, 'end_turn')
        self.assertEqual((await session.initial)['configured_session_id'], 'session')

    async def test_wrong_duplicate_malformed_and_eof_fail_closed(self):
        cases = [b'{"event":"result","message_id":"wrong","session_id":"session","complete":true}\n',
                 b'{"event":"result","message_id":"notice","session_id":"wrong","complete":true}\n',
                 b'{"event":"configured"}\n{"event":"configured"}\n', b'not-json\n', b'']
        for lines in cases:
            with self.subTest(lines=lines):
                session = agy.Session(NS(), {'pending': {'id': 'notice'}, 'session_id': 'session'})
                session.initial = asyncio.get_running_loop().create_future()
                session.turn = asyncio.get_running_loop().create_future()
                output = asyncio.StreamReader(); output.feed_data(lines); output.feed_eof()
                session.process = NS(stdout=output)
                with patch.object(agy, 'emit'):
                    await session.read()
                self.assertIsNotNone(session.failure)
                for future in [session.initial, session.turn]:
                    if future.done() and not future.cancelled():
                        future.exception()

    async def test_false_complete_stays_error_and_duplicate_result_is_failure(self):
        session = agy.Session(NS(), {'pending': {'id': 'notice'}, 'session_id': 'session'})
        session.initial = asyncio.get_running_loop().create_future()
        session.initial.set_result({})
        session.turn = asyncio.get_running_loop().create_future()
        output = asyncio.StreamReader()
        result = b'{"event":"result","message_id":"notice","session_id":"session","complete":false}\n'
        output.feed_data(result + result); output.feed_eof(); session.process = NS(stdout=output)
        with patch.object(agy, 'emit'):
            await session.read()
        self.assertEqual(await session.turn, 'error')
        self.assertIsNotNone(session.failure)


def fake_sdk():
    class Config:
        def __init__(self, **kwargs):
            self.__dict__.update(kwargs)
    types = NS(McpStdioServer=Config, CapabilitiesConfig=Config, RetryConfig=Config,
        ModelAPIRetryConfig=Config, ModelOutputRetryConfig=Config,
        SessionContinuationMode=NS(CREATE_ONLY='create', RESUME='resume'),
        StopReason=NS(UNSPECIFIED='unspecified'), StepType=NS(TEXT_RESPONSE='text'), StepStatus=NS(DONE='done'))
    modules = {}
    for name in ['google', 'google.antigravity', 'google.antigravity.connections',
        'google.antigravity.connections.connection', 'google.antigravity.connections.local',
        'google.antigravity.connections.local.local_openai_connection', 'google.antigravity.hooks']:
        modules[name] = ModuleType(name)
    modules['google.antigravity'].types = types
    modules['google.antigravity'].Agent = Mock()
    modules['google.antigravity.connections.connection'].AgentConfig = Config
    strategy = Mock()
    modules['google.antigravity.connections.local.local_openai_connection'].LocalOpenAIConnectionStrategy = strategy
    modules['google.antigravity.hooks'].policy = NS(deny_all=lambda: 'deny', allow=lambda server, tools: ('allow', server, tools))
    return modules, types, strategy


class AntigravitySDKContracts(unittest.TestCase):
    def test_configuration_forwards_binding_continuation_policy_and_exact_version(self):
        modules, types, strategy = fake_sdk()
        with patch.dict(sys.modules, modules):
            config_module = load('agy_config_fixture', 'integrations/antigravity/receiver_config.py')
        for creating, mode in [(True, 'create'), (False, 'resume')]:
            config = config_module.configuration(Path('/profile'), '/htalk', '/db', 'peer', 'model',
                                                'http://localhost:1/v1', 'session', creating)
            self.assertEqual(config.session_continuation_mode, mode)
            self.assertEqual(config.capabilities.enabled_tools, [])
            self.assertFalse(config.capabilities.enable_subagents)
            self.assertEqual(config.policies[0], 'deny')
            self.assertEqual(config.mcp_servers[0].args, ['--db', '/db', '--as', 'peer', 'mcp'])
            with patch.object(config_module, 'version', return_value='0.1.18'):
                config.create_strategy(tool_runner='tool', hook_runner='hook')
            forwarded = strategy.call_args.kwargs
            for key in ['conversation_id', 'session_continuation_mode', 'policies', 'retry_config',
                        'save_dir', 'app_data_dir', 'workspaces', 'mcp_servers']:
                self.assertEqual(forwarded[key], getattr(config, key))
            self.assertEqual(forwarded['env'], {})
            with patch.object(config_module, 'version', return_value='0.1.19'):
                with self.assertRaises(RuntimeError):
                    config.create_strategy(tool_runner='tool', hook_runner='hook')

    def test_completion_requires_new_error_free_done_native_text_and_idle(self):
        modules, types, _ = fake_sdk()
        modules['receiver_config'] = NS(configuration=Mock())
        with patch.dict(sys.modules, modules):
            worker = load('agy_worker_fixture', 'integrations/antigravity/worker.py')
        last = NS(id='new', type='text', status='done', is_complete_response=True, error=None)
        agent = NS(conversation=NS(history=[last], connection=NS(is_idle=True)))
        response = NS(stop_reason='unspecified')
        self.assertTrue(worker.complete_response(agent, response, 'old'))
        for field, value in [('id', 'old'), ('type', 'tool'), ('status', 'running'),
                             ('is_complete_response', False), ('error', 'failure')]:
            old = getattr(last, field); setattr(last, field, value)
            self.assertFalse(worker.complete_response(agent, response, 'old'))
            setattr(last, field, old)
        agent.conversation.connection.is_idle = False
        self.assertFalse(worker.complete_response(agent, response, 'old'))


class OpenHandsAdmission(unittest.TestCase):
    def test_crash_guard_exact_binding_and_explicit_consumed_recovery(self):
        with tempfile.TemporaryDirectory() as directory:
            ledger = oh.AdmissionLedger(directory)
            binding = {'conversation_id': 'session', 'peer': 'peer', 'db': '/unused', 'htalk': '/fake'}
            with patch.object(oh, 'save', wraps=managed.save), patch.object(managed, 'emit'):
                ledger.bind(binding)
                ledger.reserve('notice', 7)
                ledger.close()  # Model controller/lock loss, not a native crash.
                ledger = oh.AdmissionLedger(directory)
                try:
                    with self.assertRaisesRegex(RuntimeError, 'recovery required'):
                        ledger.preflight()
                    with self.assertRaisesRegex(RuntimeError, 'conversation differs'):
                        ledger.recover('other', 'notice', 'consumed')
                    with self.assertRaises(RuntimeError):
                        ledger.recover('session', 'wrong', 'retry')
                    ledger.recover('session', 'notice', 'consumed')
                    self.assertEqual(ledger.state['last_seq'], 7)
                    receipt = json.loads(next((Path(directory) / 'recovery').glob('*.json')).read_text())
                    self.assertEqual(receipt['pending'], {'id': 'notice', 'seq': 7})
                    with self.assertRaisesRegex(RuntimeError, 'binding differs'):
                        ledger.bind({**binding, 'peer': 'other'})
                    ledger.bind(binding)
                    ledger.reserve('helper-answer', 8)
                    ledger.consume('helper-answer', 8)
                    ledger.finish()
                    ledger.preflight()
                finally:
                    ledger.close()

    def test_retry_retains_cursor_and_no_pending_recovery_is_explicit(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(managed, 'emit'):
            ledger = oh.AdmissionLedger(directory)
            try:
                ledger.bind({'conversation_id': 'session'})
                ledger.reserve('notice', 3)
                ledger.recover('session', 'notice', 'retry')
                self.assertEqual(ledger.state['last_seq'], 0)
                ledger.bind({'conversation_id': 'session'})
                with self.assertRaises(RuntimeError):
                    ledger.recover('session', None, 'retry')
                ledger.recover('session', None, 'consumed')
                ledger.preflight()
            finally:
                ledger.close()

    def test_second_writer_is_refused_without_modifying_state(self):
        with tempfile.TemporaryDirectory() as directory:
            ledger = oh.AdmissionLedger(directory)
            try:
                with self.assertRaises(BlockingIOError):
                    oh.AdmissionLedger(directory)
            finally:
                ledger.close()




def fake_tui():
    class Message:
        def __init__(self, content): self.content = content
        def prevent_default(self): self.prevented = True
        def stop(self): self.stopped = True
    class Manager:
        async def _on_send_message(self, event):
            self.native_calls += 1
            self.run_worker('opaque native work', name='process_message')
        def run_worker(self, work, *, name):
            self.native_worker.name = name
            self.workers.append(self.native_worker)
            return self.native_worker
    app = ModuleType('openhands_cli.tui.textual_app')
    app.__file__ = '/fixture/textual_app.py'
    app.OpenHandsApp = type('App', (), {})
    modules = {'textual': NS(on=lambda *args: lambda fn: fn),
        'openhands_cli': ModuleType('openhands_cli'),
        'openhands_cli.tui': NS(textual_app=app),
        'openhands_cli.tui.core.conversation_manager': NS(ConversationManager=Manager),
        'openhands_cli.tui.messages': NS(SendMessage=Message)}
    return modules


async def until(predicate):
    async with asyncio.timeout(2):
        while not predicate():
            await asyncio.sleep(.01)


class OpenHandsNativeBoundary(unittest.IsolatedAsyncioTestCase):
    async def fixture(self, directory, *, seq=1, status='idle'):
        ledger = oh.AdmissionLedger(directory)
        with patch.dict(sys.modules, fake_tui()), patch.object(oh, 'version', side_effect=['1.16.0', '1.21.0']):
            Notice, Manager, App, _ = oh.build_tui(ledger)
        worker_done = asyncio.Event()
        worker = NS(name='process_message', is_finished=False)
        async def wait():
            await worker_done.wait()
        worker.wait = wait
        runner = NS(is_running=False, conversation=NS(state=NS(execution_status=NS(value=status))))
        manager = Manager()
        manager.state = NS(conversation_id='session')
        manager.current_runner, manager.workers = runner, []
        manager.native_worker, manager.native_calls = worker, 0
        posted = []
        def post(notice):
            posted.append(asyncio.create_task(manager._on_notice(notice)))
            return True
        manager.post_message = post
        app = App(); app.conversation_id = 'session'; app.conversation_manager = manager
        app.notify = Mock()
        output = asyncio.StreamReader()
        output.feed_data(json.dumps({'event': 'message', 'id': 'notice', 'seq': seq,
                                    'notification': 'synthetic'}).encode() + b'\n')
        child = NS(returncode=0, stdout=output)
        stack = ExitStack()
        stack.enter_context(patch.dict(os.environ, {'HTALK_PEER': 'peer', 'HTALK_DB': '/unused',
                                                  'HTALK_BIN': '/fixture/htalk'}))
        stack.enter_context(patch.object(asyncio, 'create_subprocess_exec', AsyncMock(return_value=child)))
        stack.enter_context(patch.object(managed, 'emit'))
        task = asyncio.create_task(app._receive())
        return NS(ledger=ledger, app=app, manager=manager, runner=runner, worker=worker,
                  done=worker_done, task=task, stack=stack, posted=posted, output=output)

    async def cleanup(self, f):
        try:
            if not f.task.done():
                f.task.cancel()
            try: await f.task
            except asyncio.CancelledError: pass
            await asyncio.gather(*f.posted)
        finally:
            f.ledger.close(); f.stack.close()

    async def test_admission_is_not_consumption_and_clean_restart_skips_same_unanswered_notice(self):
        with tempfile.TemporaryDirectory() as directory:
            f = await self.fixture(directory)
            try:
                await until(lambda: f.manager.native_calls == 1)
                self.assertEqual(f.ledger.state['pending'], {'id': 'notice', 'seq': 1})
                self.assertEqual(f.ledger.state['last_seq'], 0)
                f.runner.conversation.state.execution_status.value = 'finished'
                f.worker.is_finished = True; f.done.set()
                await until(lambda: f.ledger.state['pending'] is None)
                self.assertEqual(f.ledger.state['last_seq'], 1)
                f.task.cancel()
                with self.assertRaises(asyncio.CancelledError): await f.task
                await f.app.on_unmount()
                self.assertEqual(f.ledger.state['phase'], 'idle')
            finally: await self.cleanup(f)
            g = await self.fixture(directory)
            try:
                await until(lambda: g.ledger.state['phase'] == 'active')
                await asyncio.sleep(.03)
                self.assertEqual(g.manager.native_calls, 0)
                g.output.feed_data(b'{"event":"message","id":"helper","seq":2,"notification":"answer"}\n')
                await until(lambda: g.manager.native_calls == 1)
                self.assertEqual(g.ledger.state['pending'], {'id': 'helper', 'seq': 2})
            finally: await self.cleanup(g)

    async def test_pause_gate_preserved_and_native_error_keeps_recovery_required(self):
        with tempfile.TemporaryDirectory() as directory:
            f = await self.fixture(directory, status='waiting_for_confirmation')
            try:
                await until(lambda: bool(f.posted))
                self.assertEqual(f.manager.native_calls, 0)
                f.runner.conversation.state.execution_status.value = 'idle'
                await until(lambda: f.manager.native_calls == 1)
                f.runner.conversation.state.execution_status.value = 'error'
                f.worker.is_finished = True; f.done.set()
                await asyncio.wait_for(f.task, 1)
                self.assertEqual(f.ledger.state['last_seq'], 0)
                self.assertEqual(f.ledger.state['pending']['id'], 'notice')
                with self.assertRaises(RuntimeError): f.ledger.preflight()
                await f.app.on_unmount()
                self.assertEqual(f.ledger.state['phase'], 'active')
            finally: await self.cleanup(f)

    async def test_uncertain_restart_refuses_native_import_and_launch(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(managed, 'emit'):
            ledger = oh.AdmissionLedger(directory)
            ledger.bind({'conversation_id': 'session'}); ledger.reserve('notice', 1); ledger.close()
            with patch.dict(os.environ, {'HTALK_PEER': 'peer', 'HTALK_DB': '/unused',
                'HTALK_OPENHANDS_STATE': directory}), patch.object(sys, 'argv', ['receiver']), \
                    patch.object(oh, 'build_tui') as native:
                with self.assertRaisesRegex(RuntimeError, 'recovery required'): oh.main()
                native.assert_not_called()


class AntigravityCancellation(unittest.IsolatedAsyncioTestCase):
    async def test_active_response_cancellation_awaits_native_cancel_and_context_cleanup(self):
        modules, _, _ = fake_sdk()
        config = Mock()
        modules['receiver_config'] = NS(configuration=config)
        response = NS(text=AsyncMock(side_effect=asyncio.CancelledError()), cancel=AsyncMock())
        context = NS(conversation=NS(history=[]), chat=AsyncMock(return_value=response))
        class Agent:
            async def __aenter__(self): return context
            async def __aexit__(self, *args): cleanup(*args)
        cleanup = Mock()
        modules['google.antigravity'].Agent = Mock(return_value=Agent())
        with patch.dict(sys.modules, modules):
            worker = load('agy_cancel_fixture', 'integrations/antigravity/worker.py')
        transport = NS(close=Mock())
        settings = {'binding': {'htalk': '/htalk', 'db': '/unused', 'peer': 'peer',
            'model': 'model', 'base_url': 'http://localhost:1/v1'}, 'profile': '/private',
            'session_id': 'session', 'creating': True}
        async def connect(factory, pipe):
            protocol = factory()
            protocol._stream_reader.feed_data((json.dumps(settings) + '\n' +
                json.dumps({'message_id': 'notice', 'text': 'synthetic'}) + '\n').encode())
            return transport, protocol
        loop = asyncio.get_running_loop()
        with patch.object(loop, 'connect_read_pipe', side_effect=connect), patch.object(worker, 'emit') as emit:
            with self.assertRaises(asyncio.CancelledError): await worker.run()
        response.cancel.assert_awaited_once()
        self.assertEqual(cleanup.call_args.args[0], asyncio.CancelledError)
        transport.close.assert_called_once()
        self.assertEqual([call.args[0] for call in emit.call_args_list], ['configured'])


if __name__ == '__main__':
    unittest.main()
