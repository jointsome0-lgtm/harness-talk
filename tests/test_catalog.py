"""Owned catalogues, restricted mailbox calls and checked endpoint bindings."""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import sys
import uuid

from compat_support import HtalkCase
from test_mcp import McpClient

META = 'harness-talk/catalog'


class CatalogCase(HtalkCase):
    def setUp(self):
        super().setUp()
        for peer in ('alice', 'bob', 'hidden', 'carol'):
            self.htalk('peer', 'add', peer, '--harness', 'generic', '--delivery', 'pull')
        self.catalog_config = self.tmp / 'catalog.json'
        self.bob = self.publish('bob')

    def publish(self, peer, profile_id=None, name='Reviewer'):
        args = ['--as', 'alice', 'catalog', 'publish', '--config', str(self.catalog_config), peer,
                '--name', name, '--role', 'Check requested work', '--device-name', 'Known device']
        if profile_id:
            args += ['--profile-id', profile_id]
        return self.htalk(*args)

    def export(self):
        return self.htalk('catalog', 'export', '--config', str(self.catalog_config), db=False)

    def local(self):
        return McpClient(self, argv=self.argv(['--as', 'alice', 'mcp', '--catalog', str(self.catalog_config)], True))

    def expected(self, profile=None):
        d = self.export()
        return {key: d[key] for key in ('schema_version', 'device_id', 'mailbox_id', 'generation', 'sender')} | {
            'profiles': [{key: p[key] for key in ('profile_id', 'binding_id', 'peer_name')}
                         for p in d['profiles'] if not profile or p['profile_id'] == profile]}

    def remote(self, binding):
        path = self.tmp / (str(uuid.uuid4()) + '.json')
        path.write_text(json.dumps(binding))
        path.chmod(0o600)
        connector = self.argv(['--as', 'alice', 'mcp', '--catalog', str(self.catalog_config)], True)
        return McpClient(self, argv=self.argv(['mcp', '--connect', '--expect-catalog', str(path), '--', *connector], False))

    def result(self, client, *args):
        value = client.call(*args)
        self.assertFalse(value.get('isError', False), value)
        return value['structuredContent']['result']

    def test_export_contains_only_published_profiles_and_no_private_addresses(self):
        self.add_peer('native', 'codex')
        self.publish('native', name='Native reviewer')
        before = hashlib.sha256(self.db.read_bytes()).hexdigest()
        d = self.export()
        self.assertEqual({'bob', 'native'}, {p['peer_name'] for p in d['profiles']})
        text = json.dumps(d)
        for key in ('workspace', 'session_id', 'socket', 'url', 'database', 'body'):
            self.assertNotIn('"' + key + '"', text)
        self.assertTrue(all(p['runtime_status'] == 'unknown' for p in d['profiles']))
        self.assertEqual(before, hashlib.sha256(self.db.read_bytes()).hexdigest())
        self.assertEqual(0o600, self.catalog_config.stat().st_mode & 0o777)

    def test_same_display_name_keeps_distinct_profile_ids(self):
        second = self.publish('carol')
        self.assertNotEqual(self.bob['profile_id'], second['profile_id'])
        self.assertEqual(2, len(self.export()['profiles']))
        self.htalk('catalog', 'unpublish', '--config', str(self.catalog_config), self.bob['profile_id'], db=False)
        self.assertEqual(['carol'], [p['peer_name'] for p in self.export()['profiles']])
        self.assertEqual(4, len(self.htalk('peer', 'list')['peers']))

    def test_config_lock_is_distinct_and_preserves_existing_writer_lock_names(self):
        for filename in ('catalog.json', 'catalog.lock', 'catalog'):
            with self.subTest(filename=filename):
                parent = self.tmp / ('private-' + filename)
                parent.mkdir(mode=0o700)
                self.catalog_config = parent / filename
                published = self.publish('bob')
                lock_name = 'catalog.lock.lock' if filename == 'catalog.lock' else 'catalog.lock'
                lock = parent / lock_name
                self.assertEqual(0o700, parent.stat().st_mode & 0o777)
                for path in (self.catalog_config, lock):
                    self.assertEqual(0o600, path.stat().st_mode & 0o777)
                self.assertNotEqual(self.catalog_config.stat().st_ino, lock.stat().st_ino)
                self.assertEqual([published['profile_id']], [p['profile_id'] for p in self.export()['profiles']])
                before = self.catalog_config.read_bytes()
                with lock.open('r+') as holder:
                    fcntl.flock(holder, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    self.error('--as', 'alice', 'catalog', 'publish', '--config', str(self.catalog_config),
                               'carol', '--name', 'Reviewer', '--role', 'Check', error='catalog_writer_active')
                    self.assertEqual(before, self.catalog_config.read_bytes())
                    self.error('catalog', 'unpublish', '--config', str(self.catalog_config),
                               published['profile_id'], db=False, error='catalog_writer_active')
                    self.assertEqual(before, self.catalog_config.read_bytes())
                second = self.publish('carol')
                self.assertEqual({'bob', 'carol'}, {p['peer_name'] for p in self.export()['profiles']})
                self.htalk('catalog', 'unpublish', '--config', str(self.catalog_config),
                           second['profile_id'], db=False)
                self.assertEqual(['bob'], [p['peer_name'] for p in self.export()['profiles']])

    def test_republish_and_retirement_invalidate_selected_binding(self):
        old = self.expected(self.bob['profile_id'])
        client = self.remote(old)
        self.result(client, 'inbox')
        self.publish('bob', profile_id=self.bob['profile_id'])
        fresh = self.expected(self.bob['profile_id'])
        self.assertNotEqual(old['profiles'][0]['binding_id'], fresh['profiles'][0]['binding_id'])
        self.assertTrue(client.call('send', 'bob', '--id', str(uuid.uuid4()), '--message', 'stale')['isError'])
        self.assertEqual(0, self.sql('SELECT count(*) FROM messages')[0][0])
        client.close()
        current = self.remote(fresh)
        self.result(current, 'send', 'bob', '--id', str(uuid.uuid4()), '--message', 'current binding')
        self.assertEqual(1, self.sql('SELECT count(*) FROM messages')[0][0])
        current.close()
        self.htalk('peer', 'retire', 'bob')
        self.assertEqual('retired', self.export()['profiles'][0]['binding_state'])

    def test_publish_cannot_change_fixed_sender_or_mailbox(self):
        before = self.catalog_config.read_bytes()
        self.error('--as', 'carol', 'catalog', 'publish', '--config', str(self.catalog_config),
                   'bob', '--name', 'Reviewer', '--role', 'Check', error='catalog_endpoint_binding_mismatch')
        other = self.tmp / 'other.sqlite3'
        shutil.copyfile(self.db, other)
        self.error('--db', str(other), '--as', 'alice', 'catalog', 'publish',
                   '--config', str(self.catalog_config), 'bob', '--name', 'Reviewer',
                   '--role', 'Check', db=False, error='catalog_endpoint_binding_mismatch')
        self.assertEqual(before, self.catalog_config.read_bytes())

    def test_raw_binding_edit_blocks_open_client_but_descriptions_do_not(self):
        client = self.remote(self.expected(self.bob['profile_id']))
        config = json.loads(self.catalog_config.read_text())
        config['profiles'][0]['display_name'] = 'Updated description'
        config['profiles'][0]['role'] = 'Updated role'
        self.catalog_config.write_text(json.dumps(config))
        self.result(client, 'inbox')
        config['profiles'][0]['binding_id'] = str(uuid.uuid4())
        self.catalog_config.write_text(json.dumps(config))
        blocked = client.call('send', 'bob', '--id', str(uuid.uuid4()), '--message', 'stale')
        self.assertTrue(blocked['isError'])
        self.assertIn('before sending', json.dumps(blocked))
        self.assertEqual(0, self.sql('SELECT count(*) FROM messages')[0][0])
        client.close()

    def test_lost_catalogue_write_response_is_recovered_without_duplicate(self):
        mode = self.tmp / 'link-mode'
        mode.write_text('drop')
        attempts = self.tmp / 'attempts'
        connector = self.tmp / 'drop-response.py'
        connector.write_text('''import json, os, subprocess, sys
from pathlib import Path
command = json.loads(sys.argv[3])
if Path(sys.argv[1]).read_text() == 'online':
    os.execv(command[0], command)
child = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
try:
    for line in sys.stdin:
        frame = json.loads(line)
        child.stdin.write(line)
        child.stdin.flush()
        if 'id' not in frame:
            continue
        response = child.stdout.readline()
        if frame['method'] == 'tools/call':
            with open(sys.argv[2], 'a') as out:
                out.write(json.dumps(frame['params']) + '\\n')
            assert not json.loads(response)['result'].get('isError', False)
            break  # Real catalogue call committed; do not forward its response.
        sys.stdout.write(response)
        sys.stdout.flush()
finally:
    child.stdin.close()
    child.wait(timeout=10)
''')
        expected = self.tmp / 'expected.json'
        expected.write_text(json.dumps(self.expected(self.bob['profile_id'])))
        expected.chmod(0o600)
        endpoint = self.argv(['--as', 'alice', 'mcp', '--catalog', str(self.catalog_config)], True)
        argv = self.argv(['mcp', '--connect', '--expect-catalog', str(expected), '--',
                          sys.executable, str(connector), str(mode), str(attempts), json.dumps(endpoint)], False)
        client = McpClient(self, argv=argv)
        request_id = str(uuid.uuid4())
        lost = client.call('send', 'bob', '--id', request_id, '--message', 'one request')
        self.assertTrue(lost['isError'])
        self.assertIn('outcome is unknown', json.dumps(lost))
        self.assertEqual(1, len(attempts.read_text().splitlines()))
        self.assertEqual(1, self.sql('SELECT count(*) FROM messages')[0][0])
        mode.write_text('online')
        original = self.result(client, 'show', request_id)
        retry = self.result(client, 'send', 'bob', '--id', request_id, '--message', 'one request')
        self.assertFalse(retry['created'])
        self.assertEqual((request_id, original['created_at']), (retry['id'], retry['created_at']))
        conflict = client.call('send', 'bob', '--id', request_id, '--message', 'changed request')
        self.assertTrue(conflict['isError'])
        self.assertIn('message_id_conflict', json.dumps(conflict))
        self.assertEqual('one request', self.result(client, 'show', request_id)['body'])
        self.assertEqual(1, self.sql('SELECT count(*) FROM messages')[0][0])
        client.close()

    def test_export_refuses_legacy_schema_without_migrating(self):
        self.sql('PRAGMA user_version=2')
        before = self.db.read_bytes()
        self.error('catalog', 'export', '--config', str(self.catalog_config), db=False, error='catalog_requires_schema3')
        self.assertEqual(2, self.user_version())
        self.assertEqual(before, self.db.read_bytes())
        self.assertFalse(Path(str(self.db) + '.backups').exists())

    def test_private_config_and_replaced_mailbox_are_rejected(self):
        self.catalog_config.chmod(0o644)
        self.error('catalog', 'export', '--config', str(self.catalog_config), db=False,
                   error='catalog_config_must_be_private_owned_file')
        self.catalog_config.chmod(0o600)
        replacement = self.tmp / 'replacement.sqlite3'
        shutil.copyfile(self.db, replacement)
        os.replace(replacement, self.db)
        self.error('catalog', 'export', '--config', str(self.catalog_config), db=False, error='catalog_mailbox_replaced')

    def test_gateway_accepts_exact_commands_only(self):
        for command in ('', 'catalog; peer list', 'mcp --as hidden', ' catalog', 'catalog\n'):
            result = self.run_raw('catalog', 'serve', '--config', str(self.catalog_config), db=False,
                                  env={'SSH_ORIGINAL_COMMAND': command})
            self.assertEqual(2, result.code)
            self.assertEqual('', result.stdout)
            self.assertIn('catalog_remote_command_refused', result.stderr)
        exported = self.htalk('catalog', 'serve', '--config', str(self.catalog_config), db=False,
                              env={'SSH_ORIGINAL_COMMAND': 'catalog'})
        self.assertEqual(self.export(), exported)
        self.assertEqual(0, self.sql('SELECT count(*) FROM messages')[0][0])

    def test_published_scope_filters_conversations_before_pagination(self):
        hidden = self.htalk('--as', 'hidden', 'send', 'alice', '--message', 'private')
        bob1 = self.htalk('--as', 'bob', 'send', 'alice', '--message', 'first')
        self.htalk('--as', 'hidden', 'send', 'alice', '--message', 'private two')
        bob2 = self.htalk('--as', 'bob', 'send', 'alice', '--message', 'second')
        private_out = self.htalk('--as', 'alice', 'send', 'hidden', '--message', 'private outgoing')
        client = self.local()
        peers = self.result(client, 'peer', 'list')['peers']
        self.assertEqual(['bob'], [p['name'] for p in peers])
        self.assertNotIn('session_id', json.dumps(peers))
        page = self.result(client, 'inbox', '--limit', '1')
        self.assertEqual((2, 1), (page['total'], page['omitted']))
        self.assertEqual([bob1['id']], [m['id'] for m in page['messages']])
        page2 = self.result(client, 'inbox', '--after-seq', str(bob1['seq']), '--limit', '1')
        self.assertEqual([bob2['id']], [m['id'] for m in page2['messages']])
        self.assertEqual(0, self.result(client, 'sent')['total'])
        before = self.sql('SELECT id,ack_at,wait_returned_at FROM messages ORDER BY seq')
        for words in [('peer', 'check', 'hidden'), ('show', hidden['id']), ('ack', hidden['id']),
                      ('reply', hidden['id'], '--message', 'blocked'), ('wait', private_out['id'], '--seconds', '0'),
                      ('send', 'hidden', '--id', str(uuid.uuid4()), '--message', 'blocked')]:
            self.assertTrue(client.call(*words)['isError'], words)
        missing = client.call('show', str(uuid.uuid4()))
        self.assertEqual(missing['content'], client.call('show', hidden['id'])['content'])
        self.assertEqual(before, self.sql('SELECT id,ack_at,wait_returned_at FROM messages ORDER BY seq'))
        client.close()

    def test_selected_proxy_cannot_address_another_published_peer(self):
        self.publish('carol')
        client = self.remote(self.expected(self.bob['profile_id']))
        self.assertEqual(['bob'], [p['name'] for p in self.result(client, 'peer', 'list')['peers']])
        self.assertTrue(client.call('send', 'carol', '--id', str(uuid.uuid4()), '--message', 'wrong profile')['isError'])
        saved = self.result(client, 'send', 'bob', '--id', str(uuid.uuid4()), '--message', 'question')
        self.assertEqual('alice', saved['sender'])
        self.assertEqual('bob', saved['recipient'])
        reply = self.htalk('--as', 'bob', 'reply', saved['id'], '--message', 'answer')
        shown = self.result(client, 'show', saved['id'])
        self.assertEqual(reply['id'], shown['reply']['id'])
        self.result(client, 'ack', reply['id'])
        self.assertEqual(2, self.sql('SELECT count(*) FROM messages')[0][0])
        client.close()

    def test_issued_pages_preserve_limit_body_mode_order_and_profile_scope(self):
        self.publish('carol')
        messages = {'inbox': [], 'sent': []}
        for n in range(3):
            for peer in ('bob', 'hidden', 'carol'):
                incoming = self.htalk('--as', peer, 'send', 'alice', '--message', f'{peer} inbox {n}\nfull body')
                outgoing = self.htalk('--as', 'alice', 'send', peer, '--message', f'{peer} sent {n}\nfull body')
                messages['inbox'].append(incoming)
                messages['sent'].append(outgoing)
        before = self.sql('SELECT id,ack_at,wait_returned_at FROM messages ORDER BY seq')
        for selected in (False, True):
            client = self.remote(self.expected(self.bob['profile_id'])) if selected else self.local()
            for command, bodies in (('inbox', False), ('sent', False), ('sent', True)):
                with self.subTest(selected=selected, command=command, bodies=bodies):
                    scope = {'bob'} if selected else {'bob', 'carol'}
                    other = 'recipient' if command == 'sent' else 'sender'
                    expected = sorted((m for m in messages[command] if m[other] in scope),
                                      key=lambda m: m['seq'], reverse=command == 'sent')
                    args = [command, '--limit', '1'] + (['--bodies'] if bodies else [])
                    for index, message in enumerate(expected):
                        page = self.result(client, *args)
                        self.assertEqual((len(expected), len(expected) - index - 1),
                                         (page['total'], page['omitted']))
                        self.assertEqual([message['id']], [m['id'] for m in page['messages']])
                        row = page['messages'][0]
                        if command == 'inbox' or bodies:
                            self.assertEqual(message['body'], row['body'])
                            self.assertNotIn('body_preview', row)
                        else:
                            self.assertNotIn('body', row)
                            self.assertEqual(message['body'].splitlines()[0], row['body_preview'])
                        if page['omitted']:
                            continuation = page['recovery']['next_page']
                            self.assertNotIn(str(self.tmp), continuation)
                            self.assertNotIn('--db', continuation)
                            self.assertNotIn('--as', continuation)
                            words = shlex.split(continuation)
                            self.assertEqual('htalk', words[0])
                            args = words[1:]
                        else:
                            self.assertNotIn('recovery', page)
            self.assertTrue(client.call('send', 'hidden', '--id', str(uuid.uuid4()), '--message', 'blocked')['isError'])
            if selected:
                self.assertTrue(client.call('send', 'carol', '--id', str(uuid.uuid4()), '--message', 'blocked')['isError'])
            client.close()
        self.assertEqual(before, self.sql('SELECT id,ack_at,wait_returned_at FROM messages ORDER BY seq'))

    def test_mismatched_metadata_refuses_write_before_call(self):
        for key in ('device_id', 'mailbox_id', 'generation', 'sender', 'profile_id', 'binding_id', 'peer_name'):
            expected = self.expected()
            obj = expected['profiles'][0] if key in expected['profiles'][0] else expected
            obj[key] = 'hidden' if key in ('sender', 'peer_name') else str(uuid.uuid4())
            client = self.remote(expected)
            reply = client.call('send', 'bob', '--id', str(uuid.uuid4()), '--message', 'blocked')
            self.assertTrue(reply['isError'], key)
            self.assertIn('before sending', json.dumps(reply))
            self.assertEqual(0, self.sql('SELECT count(*) FROM messages')[0][0])
            client.close()

    def test_unavailable_interface_is_not_an_empty_success(self):
        trust = self.tmp / 'empty-trust.json'
        trust.write_text(json.dumps({'schema_version':1,'devices':[]}))
        trust.chmod(0o600)
        self.error('catalog','discover','--trust',str(trust),'--interface','htalk-missing0','--seconds','1',
                   db=False,error='catalog_interface_not_found')
        self.error('catalog','discover','--trust',str(trust),'--interface','lo','--seconds','1',
                   db=False,error='catalog_interface_unavailable')

    def test_endpoint_without_catalogue_metadata_refuses_write(self):
        path = self.tmp / 'expected.json'
        path.write_text(json.dumps(self.expected()))
        path.chmod(0o600)
        connector = self.argv(['--as', 'alice', 'mcp'], True)
        client = McpClient(self, argv=self.argv(['mcp', '--connect', '--expect-catalog', str(path), '--', *connector], False))
        result = client.call('send', 'bob', '--id', str(uuid.uuid4()), '--message', 'blocked')
        self.assertTrue(result['isError'])
        self.assertIn('before sending', json.dumps(result))
        self.assertEqual(0, self.sql('SELECT count(*) FROM messages')[0][0])
        client.close()

    def test_local_catalog_change_blocks_existing_client(self):
        client = self.local()
        self.result(client, 'inbox')
        self.htalk('catalog', 'unpublish', '--config', str(self.catalog_config), self.bob['profile_id'], db=False)
        self.assertTrue(client.call('send', 'bob', '--id', str(uuid.uuid4()), '--message', 'removed')['isError'])
        self.assertEqual(0, self.sql('SELECT count(*) FROM messages')[0][0])
        client.close()
