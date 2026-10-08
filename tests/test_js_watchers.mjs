import assert from 'node:assert/strict';
import test from 'node:test';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createInterface } from 'node:readline';
import { setImmediate as immediate } from 'node:timers/promises';
import { FakeChild, fakeProcess, readlineMock, load, until } from './js_watchers_support.mjs';

async function clineFixture({ snapshotDetach = false, subscribeDetach = false, retry = false } = {}) {
  const p = fakeProcess({ HTALK_DB: '/mock/unused.db', HTALK_SESSION: 'session',
    HTALK_WORKSPACE: '/mock/workspace', HTALK_CLINE_ROOT: '/mock/cline' });
  const child = new FakeChild({ autoClose: true });
  const interfaces = [], trace = [], calls = [];
  let subscription, attached = true, spawned = false, disposals = 0, dispatches = 0;
  const detach = (clientId = 'terminal', event = 'session.detached', sessionId = 'session') => {
    if (clientId === 'terminal' && sessionId === 'session') attached = false;
    subscription?.({ event, sessionId, payload: { clientId } });
  };
  const hub = {
    resolveProductionHubOwnerContext: () => ({ discoveryPath: '/mock/discovery' }),
    readHubDiscovery: async () => ({ url: 'ws://127.0.0.1:1234', authToken: 'fake' }),
    NodeHubClient: class {
      async connect() { trace.push('connect'); }
      subscribe(callback) {
        trace.push('subscribe'); subscription = callback;
        if (subscribeDetach) detach();
        return () => { subscription = undefined; };
      }
      async command(name, params, sessionId, options) {
        trace.push(name); calls.push({ name, params, sessionId });
        if (name === 'session.list') return { ok: true, payload: { sessions: [{
          sessionId: 'session', workspaceRoot: '/mock/workspace', status: 'idle',
          participants: attached ? [{ clientId: 'terminal' }] : [],
        }] } };
        if (name === 'client.list') {
          const result = { ok: true, payload: { clients: attached ? [{ clientId: 'terminal',
            clientType: 'cli', workspaceContext: { workspaceRoot: '/mock/workspace' } }] : [] } };
          if (snapshotDetach && attached) detach();
          return result;
        }
        if (name === 'run.enqueue') {
          options.beforeDispatch(); dispatches++;
          if (retry) options.beforeDispatch();
          return { ok: true, payload: { runId: 'run' } };
        }
        throw Error('unexpected_command');
      }
      async dispose() { disposals++; }
    },
  };
  const module = await load('integrations/cline.mjs', {
    'node:child_process': { spawn: () => { spawned = true; trace.push('spawn'); return child; } },
    'node:fs': { readFileSync: () => JSON.stringify({ version: '0.0.86' }) },
    'node:path': { resolve }, 'node:url': { pathToFileURL },
    'node:readline': readlineMock(interfaces),
  }, p, { dynamicImport: () => hub });
  const done = module.evaluate();
  return { p, child, done, trace, calls, detach, get spawned() { return spawned; },
    get disposals() { return disposals; }, get dispatches() { return dispatches; } };
}

for (const mode of ['snapshotDetach', 'subscribeDetach']) {
  test(`Cline refuses startup when terminal detaches during ${mode}`, { timeout: 6000 }, async () => {
    const f = await clineFixture({ [mode]: true });
    await f.done;
    assert.equal(f.spawned, false);
    assert.equal(f.p.records.some(e => e.event === 'ready'), false);
    assert.equal(f.trace.indexOf('subscribe') < f.trace.indexOf('session.list'), true);
    assert.equal(f.disposals, 1);
    assert.equal(f.p.exitCode, 1);
  });
}

test('Cline preserves enqueue arguments, ignores unrelated detach and stops on a bound detach', { timeout: 6000 }, async () => {
  const f = await clineFixture();
  try {
    await until(() => f.spawned, 'Cline watcher');
    f.child.line({ event: 'ready', peer: 'mock-peer' });
    await until(() => f.p.records.some(e => e.event === 'ready'), 'ready');
    f.detach('other-terminal');
    f.detach('terminal', 'session.detached', 'other-session');
    f.child.line({ event: 'message', id: 'id', notification: 'notice' });
    await until(() => f.p.records.some(e => e.submission === 'submitted'), 'enqueue receipt');
    const call = f.calls.find(e => e.name === 'run.enqueue');
    assert.equal(JSON.stringify(call.params), JSON.stringify({ prompt: 'notice', delivery: 'queue' }));
    assert.equal(call.sessionId, 'session');
    assert.equal(f.dispatches, 1);
    f.detach('terminal', 'hub.client.disconnected');
    await f.done;
    assert.equal(f.p.records.some(e => e.event === 'stopped' && e.reason === 'terminal_detached'), true);
    assert.equal(f.child.closed, true);
    assert.deepEqual(f.child.signals, ['SIGTERM']);
    assert.equal(f.disposals, 1);
    const receipts = f.p.records.length;
    // The SDK may replay a detach after reconnect or queue one during stop.
    f.detach('terminal', 'hub.client.disconnected');
    f.detach();
    assert.equal(f.p.records.length, receipts);
  } finally { f.p.emit('SIGTERM'); await f.done; }
});

test('Cline keeps uncertain dispatch receipt and refuses an SDK retry', { timeout: 6000 }, async () => {
  const f = await clineFixture({ retry: true });
  try {
    await until(() => f.spawned, 'Cline watcher');
    f.child.line({ event: 'message', id: 'id', notification: 'notice' });
    await f.done;
    assert.equal(f.dispatches, 1);
    assert.equal(f.p.records.filter(e => e.event === 'notice').length, 1);
    assert.equal(f.p.records.find(e => e.event === 'notice').submission, 'submission_unknown');
    assert.equal(f.child.closed, true);
  } finally { f.p.emit('SIGTERM'); await f.done; }
});

test('Cline handles a modeled replayed bound detach before receiving further mail', { timeout: 6000 }, async () => {
  const f = await clineFixture();
  try {
    await until(() => f.spawned, 'Cline watcher');
    // Represent the SDK delivering its saved detach event after reconnect.
    // This does not create a socket or verify SDK transport/replay scheduling.
    f.detach();
    await f.done;
    assert.equal(f.child.closed, true);
    assert.equal(f.dispatches, 0);
    assert.equal(f.p.records.filter(e => e.event === 'stopped').length, 1);
  } finally { f.p.emit('SIGTERM'); await f.done; }
});

async function piFixture({ spawnChild = () => new FakeChild(), realReadline = false, timers } = {}) {
  const p = fakeProcess(), children = [], interfaces = [], handlers = {}, notices = [], messages = [];
  const module = await load('integrations/pi.ts', {
    'node:child_process': { spawn: (executable, args, options) => {
      assert.equal(executable, 'mock-htalk');
      assert.equal(JSON.stringify(args), JSON.stringify(['--as', 'mock-peer', 'watch']));
      const child = spawnChild(options); children.push(child); return child;
    } },
    'node:readline': realReadline ? { createInterface } : readlineMock(interfaces),
  }, p, { ts: true, timers });
  await module.evaluate();
  module.module.namespace.default({ on: (name, fn) => { handlers[name] = fn; },
    sendMessage: (...args) => messages.push(args) });
  const ctx = { ui: { notify: (...args) => notices.push(args) } };
  return { p, children, interfaces, handlers, notices, messages, ctx,
    start: () => handlers.session_start({}, ctx), shutdown: () => handlers.session_shutdown() };
}

function manualTimers() {
  const pending = new Set();
  return { pending, setTimeout(callback, ms) {
    assert.ok([2000, 4000].includes(ms));
    const timer = { callback, ms, unref() {} }; pending.add(timer); return timer;
  }, clearTimeout(timer) { pending.delete(timer); }, fire(ms = 2000) {
    for (const timer of [...pending]) if (timer.ms === ms) timer.callback();
  } };
}

test('Pi shutdown awaits close, suppresses stale callbacks and delays replacement', { timeout: 6000 }, async () => {
  const timers = manualTimers(), f = await piFixture({ timers });
  await f.start();
  const old = f.children[0];
  old.line({ event: 'ready' });
  old.line({ event: 'message', notification: 'one' });
  assert.equal(JSON.stringify(f.messages[0]), JSON.stringify([
    { customType: 'htalk', content: 'one', display: true }, { triggerTurn: true, deliverAs: 'followUp' },
  ]));
  let completed = false;
  const shutdown = f.shutdown().then(() => { completed = true; });
  const restart = f.start();
  await immediate();
  assert.equal(completed, false);
  assert.equal(f.children.length, 1);
  assert.equal(f.interfaces[0].closed, true);
  old.line({ event: 'message', notification: 'stale' });
  old.emit('error', Error('stale'));
  assert.equal(f.messages.length, 1);
  assert.equal(f.notices.length, 1);
  assert.deepEqual(old.signals, ['SIGTERM']);
  timers.fire();
  assert.deepEqual(old.signals, ['SIGTERM', 'SIGKILL']);
  old.finish('SIGKILL');
  await Promise.all([shutdown, restart]);
  assert.equal(completed, true);
  assert.equal(f.children.length, 2);
  assert.equal(timers.pending.size, 0);
  f.children[1].line({ event: 'message', notification: 'two' });
  assert.equal(f.messages.length, 2);
  const done = f.shutdown(); f.children[1].finish(); await done;
});

test('Pi ignores superseded starts, including shutdown during a pending start', { timeout: 6000 }, async () => {
  const f = await piFixture();
  await f.start();
  const first = f.start(), second = f.start();
  f.children[0].finish();
  await Promise.all([first, second]);
  assert.equal(f.children.length, 2);
  const pending = f.start(), shutdown = f.shutdown();
  f.children[1].finish();
  await Promise.all([pending, shutdown]);
  assert.equal(f.children.length, 2);
  delete f.p.env.HTALK_PEER;
  await f.start();
  assert.equal(f.children.length, 2);
});

test('Pi handles asynchronous spawn failure through close without stale notices', { timeout: 6000 }, async () => {
  const f = await piFixture({ spawnChild: () => { const c = new FakeChild(); c.pid = undefined; return c; } });
  await f.start();
  const child = f.children[0];
  child.emit('error', Error('ENOENT'));
  const shutdown = f.shutdown();
  await immediate();
  assert.equal(f.notices.length, 1);
  assert.equal(f.notices[0][1], 'error');
  assert.deepEqual(child.signals, []);
  assert.equal(f.interfaces[0].closed, true);
  child.finish(); await shutdown;
  child.line({ event: 'message', notification: 'stale' });
  assert.equal(f.messages.length, 0);
});

test('Pi bounds failed termination and refuses replacement while closure is uncertain', { timeout: 6000 }, async () => {
  const timers = manualTimers(), f = await piFixture({ timers });
  await f.start();
  const child = f.children[0];
  const shutdown = f.shutdown();
  const rejected = assert.rejects(shutdown, /did not close within 4 seconds/);
  timers.fire(2000);
  timers.fire(4000);
  await rejected;
  assert.deepEqual(child.signals, ['SIGTERM', 'SIGKILL']);
  assert.equal(child.closed, false);
  assert.equal(child.stdout.destroyed, true);
  assert.equal(child.stderr.destroyed, true);
  await assert.rejects(f.start(), /replacement refused/);
  assert.equal(f.children.length, 1);
  assert.equal(timers.pending.size, 0);
  child.finish('SIGKILL');
});

test('Pi closes its pipe readers after child exit even if inherited writers delay close', { timeout: 6000 }, async () => {
  const f = await piFixture();
  await f.start();
  const child = f.children[0], shutdown = f.shutdown();
  child.exitCode = 0;
  child.emit('exit', 0, null);
  assert.equal(child.stdout.destroyed, true);
  assert.equal(child.stderr.destroyed, true);
  child.finish(); await shutdown;
  const g = await piFixture();
  await g.start();
  const exited = g.children[0];
  exited.exitCode = 0;
  // Natural exit calls fail(), which begins stop only after this exit event.
  exited.emit('exit', 0, null);
  assert.equal(exited.stdout.destroyed, true);
  assert.equal(exited.stderr.destroyed, true);
  const stopped = g.shutdown();
  exited.finish(); await stopped;
  assert.equal(g.notices.length, 1);
});

for (const stubborn of [false, true]) {
  test(`Pi reaps an owned ${stubborn ? 'SIGTERM-ignoring' : 'cooperative'} OS child before shutdown resolves`, { timeout: 8000 }, async () => {
    // This local Node child writes NDJSON and holds a timer. It is not htalk/Pi.
    // The VM replaces only spawn and process.env; readline and timers are real.
    const script = `process.on('SIGTERM', () => ${stubborn ? '{}' : 'process.exit(0)'});\n` +
      `process.stdout.write(JSON.stringify({event:'ready'})+'\\n');setInterval(()=>{},1000);`;
    const f = await piFixture({ realReadline: true,
      spawnChild: options => spawn(process.execPath, ['-e', script], options) });
    let child;
    try {
      await f.start(); child = f.children[0];
      await until(() => f.notices.length === 1, 'owned child ready');
      const closing = once(child, 'close');
      const started = Date.now();
      await f.shutdown();
      await closing;
      const elapsed = Date.now() - started;
      if (process.platform === 'win32') {
        // Node has no request to stop there: the first signal ends the child, whatever it would do.
        assert.equal(child.signalCode, 'SIGTERM');
      } else {
        assert.equal(child.signalCode, stubborn ? 'SIGKILL' : null);
        if (stubborn) assert.ok(elapsed >= 1900 && elapsed < 5000, `termination took ${elapsed}ms`);
        else assert.equal(child.exitCode, 0);
      }
      assert.equal(f.notices.length, 1);
      assert.throws(() => process.kill(child.pid, 0), { code: 'ESRCH' });
    } finally {
      if (child && child.exitCode === null && child.signalCode === null) {
        const closed = once(child, 'close'); child.kill('SIGKILL'); await closed;
      }
      await f.shutdown();
    }
  });
}
