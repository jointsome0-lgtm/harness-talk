// Adapters execute in a VM with enumerated imports and a synthetic process.
// No native host package, mailbox, credentials or model is accessed.
import { EventEmitter } from 'node:events';
import { readFileSync } from 'node:fs';
import { SourceTextModule, SyntheticModule, createContext } from 'node:vm';
import { stripTypeScriptTypes } from 'node:module';
import { resolve } from 'node:path';
import { setImmediate as immediate } from 'node:timers/promises';
const root = resolve(import.meta.dirname, '..');

export function fakeProcess(extraEnv = {}) {
  const p = new EventEmitter();
  p.env = { HTALK_PEER: 'mock-peer', HTALK_BIN: 'mock-htalk', ...extraEnv };
  p.records = [];
  p.stdout = { write(text) { p.records.push(JSON.parse(text)); } };
  p.exitCode = undefined;
  return p;
}

export class FakeChild extends EventEmitter {
  constructor({ autoClose = false } = {}) {
    super();
    this.pid = 1000;
    this.exitCode = null;
    this.signalCode = null;
    this.stdout = new EventEmitter();
    this.stdout.destroy = () => { this.stdout.destroyed = true; };
    this.stderr = new EventEmitter();
    this.stderr.destroy = () => { this.stderr.destroyed = true; };
    this.stderr.resume = () => { this.stderr.drained = true; };
    this.signals = [];
    this.autoClose = autoClose;
    this.closed = false;
  }
  kill(signal = 'SIGTERM') {
    this.signals.push(signal);
    if (this.autoClose) queueMicrotask(() => this.finish(signal));
    return true;
  }
  finish(signal = 'SIGTERM') {
    if (this.closed) return;
    this.closed = true;
    this.signalCode = signal;
    this.stdout.emit('end');
    this.emit('exit', null, signal);
    this.emit('close', null, signal);
  }
  line(object) { this.stdout.emit('mockLine', JSON.stringify(object)); }
}

export function readlineMock(interfaces) {
  return { createInterface({ input }) {
    const lines = new EventEmitter();
    const pending = [];
    const waiting = [];
    lines.closed = false;
    lines.close = () => {
      lines.closed = true;
      for (const resolveNext of waiting.splice(0)) resolveNext({ done: true });
    };
    input.on('mockLine', line => {
      if (lines.closed) return;
      lines.emit('line', line);
      if (waiting.length) waiting.shift()({ value: line, done: false });
      else pending.push(line);
    });
    input.on('end', lines.close);
    lines[Symbol.asyncIterator] = () => ({
      next() {
        if (pending.length) return Promise.resolve({ value: pending.shift(), done: false });
        if (lines.closed) return Promise.resolve({ done: true });
        return new Promise(resolveNext => waiting.push(resolveNext));
      },
      return() { lines.close(); return Promise.resolve({ done: true }); },
    });
    interfaces.push(lines);
    return lines;
  } };
}

export async function load(path, imports, p, { ts = false, dynamicImport, timers = { setTimeout, clearTimeout } } = {}) {
  // A dedicated context makes process fake while preserving standard JS globals.
  const context = createContext({ process: p, URL, ...timers, console: undefined });
  const cached = new Map();
  const makeSynthetic = values => {
    const names = Object.keys(values);
    return new SyntheticModule(names, function () {
      for (const name of names) this.setExport(name, values[name]);
    }, { context });
  };
  const module = new SourceTextModule(ts ? stripTypeScriptTypes(readFileSync(resolve(root, path), 'utf8'), { mode: 'strip' }) : readFileSync(resolve(root, path), 'utf8'), {
    context, identifier: resolve(root, path),
    importModuleDynamically: dynamicImport ? async specifier => {
      const values = dynamicImport(specifier);
      const mock = makeSynthetic(values);
      await mock.link(() => { throw Error('Unexpected import in synthetic mock'); });
      await mock.evaluate({ timeout: 1000 });
      return mock;
    } : async () => { throw Error('Dynamic production import blocked'); },
  });
  await module.link(specifier => {
    if (!Object.hasOwn(imports, specifier)) throw Error(`Production import blocked: ${specifier}`);
    if (!cached.has(specifier)) cached.set(specifier, makeSynthetic(imports[specifier]));
    return cached.get(specifier);
  });
  return { module, evaluate: () => module.evaluate({ timeout: 1000 }) };
}

export async function until(predicate, label) {
  const deadline = Date.now() + 4000;
  while (!predicate()) {
    if (Date.now() > deadline) throw Error(`Mock wait expired: ${label}`);
    await immediate();
  }
}
