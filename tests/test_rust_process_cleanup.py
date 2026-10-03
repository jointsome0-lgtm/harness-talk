"""Owned disposable connector descendants at the public Rust CLI boundary."""
import json
import os
from pathlib import Path
import select
import signal
import sys
import unittest

from compat_support import HtalkCase, process_start, wait_for
from test_mcp import McpClient


# Every descendant has a hard lifetime limit. The wrapper waits for its ready
# byte before advertising it, so TERM cannot race ahead of handler installation.
FORK = r'''
r, w = os.pipe()
pid = os.fork()
if pid == 0:
    os.close(r)
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    signal.alarm(12)
    os.write(w, b'x')
    os.close(w)
    for fd in (0, 1, 2):
        try: os.close(fd)
        except OSError: pass
    while True: time.sleep(.1)
os.close(w)
assert os.read(r, 1) == b'x'
os.close(r)
def start(pid):
    return Path('/proc/%d/stat' % pid).read_text().rpartition(')')[2].split()[19]
with records.open('a') as out:
    out.write(json.dumps({'wrapper': os.getpid(), 'pid': pid, 'start': start(pid),
                          'group': os.getpgid(pid)}) + '\n')
'''


class RustOwnedCleanup(HtalkCase):
    def setUp(self):
        super().setUp()
        self.handles = {}
        self.record_files = []
        self.addCleanup(self.cleanup_owned)

    def pin_records(self):
        for path in self.record_files:
            if not path.exists():
                continue
            for line in path.read_text().splitlines():
                record = json.loads(line)
                key = (record['pid'], record['start'])
                if key in self.handles:
                    continue
                try:
                    if process_start(record['pid']) != record['start']:
                        continue
                    fd = os.pidfd_open(record['pid'])
                    if process_start(record['pid']) != record['start']:
                        os.close(fd)
                        continue
                except (FileNotFoundError, ProcessLookupError):
                    continue
                self.handles[key] = fd
        return self.handles

    @staticmethod
    def exited(fd):
        poll = select.poll()
        poll.register(fd, select.POLLIN)
        return bool(poll.poll(0))

    def cleanup_owned(self):
        self.pin_records()
        for fd in self.handles.values():
            try:
                signal.pidfd_send_signal(fd, signal.SIGKILL)
            except ProcessLookupError:
                pass
            os.close(fd)

    def records(self, path):
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def test_mcp_complete_loss_cancel_eof_and_terminate_stop_descendants_without_replay(self):
        for mode in ('complete', 'drop', 'cancel', 'eof', 'terminate'):
            with self.subTest(mode=mode):
                records = self.tmp / ('mcp-' + mode + '.jsonl')
                self.record_files.append(records)
                attempts = self.tmp / ('attempts-' + mode)
                gate = self.tmp / ('gate-' + mode)
                script = self.tmp / ('endpoint-' + mode + '.py')
                script.write_text('import json, os, signal, sys, time\nfrom pathlib import Path\n'
                                  f'records = Path({str(records)!r})\n'
                                  'for line in sys.stdin:\n'
                                  '    d = json.loads(line)\n'
                                  "    if d['method'] == 'initialize':\n"
                                  "        result = {'protocolVersion': d['params']['protocolVersion'], 'capabilities': {'tools': {}}, 'serverInfo': {'name': 'harness-talk', 'version': 'fixture'}}\n"
                                  "        print(json.dumps({'jsonrpc': '2.0', 'id': d['id'], 'result': result}), flush=True)\n"
                                  "    elif d['method'] == 'tools/call':\n" +
                                  ''.join('        ' + line + '\n' for line in FORK.strip().splitlines()) +
                                  f"        Path({str(attempts)!r}).open('a').write('one call\\n')\n" +
                                  f"        while not Path({str(gate)!r}).exists(): time.sleep(.01)\n" +
                                  ("        print(json.dumps({'jsonrpc': '2.0', 'id': d['id'], 'result': {'content': []}}), flush=True)\n        sys.exit(0)\n" if mode == 'complete' else
                                   '        sys.exit(0)\n' if mode == 'drop' else
                                   '        sys.stdin.read()\n        sys.exit(0)\n'))
                client = McpClient(self, connector=[sys.executable, str(script)])
                client.send('tools/call', {'name': 'htalk', 'arguments': {'args': ['inbox']}}, 100)
                wait_for(lambda: records.exists() and attempts.exists())
                self.pin_records()
                gate.touch()
                if mode in ('complete', 'drop'):
                    while True:
                        response = client.responses.get(timeout=15)
                        if response.get('id') == 100:
                            break
                    result = response['result']
                    self.assertEqual(mode == 'drop', bool(result.get('isError')))
                    if mode == 'drop':
                        self.assertIn('outcome is unknown', result['content'][0]['text'])
                elif mode == 'cancel':
                    client.send('notifications/cancelled', {'requestId': 100, 'reason': 'fixture'})
                elif mode == 'eof':
                    client.close()
                else:
                    client.process.terminate()
                    client.process.wait(timeout=15)
                    self.assertEqual(0, client.process.returncode, client.process.stderr.read())
                self.pin_records()
                record = self.records(records)[0]
                self.assertEqual(record['wrapper'], record['group'])
                fd = self.handles[(record['pid'], record['start'])]
                wait_for(lambda: self.exited(fd), timeout=5, message='owned MCP descendant exit')
                self.assertEqual(['one call'], attempts.read_text().splitlines())
                if mode in ('complete', 'drop', 'cancel'):
                    self.assertIn('result', client.request('ping'))
                    client.close()

    def test_receiver_cancel_and_watch_eof_stop_descendants_before_reconnect(self):
        peer = self.codex_recipient('alice')
        for mode in ('cancel', 'eof'):
            with self.subTest(mode=mode):
                records = self.tmp / ('receive-' + mode + '.jsonl')
                self.record_files.append(records)
                gate = self.tmp / ('watch-gate-' + mode)
                script = self.tmp / ('watch-' + mode + '.py')
                script.write_text('import json, os, signal, sys, time\nfrom pathlib import Path\n'
                                  f'records = Path({str(records)!r})\n' + FORK +
                                  f'while not Path({str(gate)!r}).exists(): time.sleep(.01)\n' +
                                  "print('{\"event\":\"ready\",\"peer\":\"alice\"}', flush=True)\n" +
                                  ('sys.stdin.read()\n' if mode == 'cancel' else ''))
                process = self.spawn('receive', '--peer', 'alice', '--session', peer['session_id'],
                                     '--workspace', str(self.work), '--state', str(self.tmp / ('state-' + mode)),
                                     '--', sys.executable, str(script), db=False)
                wait_for(lambda: self.records(records))
                self.pin_records()
                first = self.records(records)[0]
                self.assertEqual(first['wrapper'], first['group'])
                first_fd = self.handles[(first['pid'], first['start'])]
                gate.touch()
                if mode == 'eof':
                    wait_for(lambda: len(self.records(records)) >= 2)
                    self.assertTrue(self.exited(first_fd), 'previous descendant alive when reconnect started')
                    self.pin_records()
                process.terminate()
                stdout, stderr = process.communicate(timeout=15)
                self.assertEqual(0, process.returncode, stdout + stderr)
                self.pin_records()
                for record in self.records(records):
                    fd = self.handles[(record['pid'], record['start'])]
                    self.assertTrue(self.exited(fd), record)
                self.assertEqual([], self.calls('codex', ['queue']))
                saved = json.loads((self.tmp / ('state-' + mode) / 'state.json').read_text())
                self.assertIsNone(saved['pending'])
                self.assertEqual({}, saved['receipts'])


if __name__ == '__main__':
    unittest.main()
