"""Owned catalogues, restricted mailbox calls and checked endpoint bindings."""
import hashlib
import json
import os
from pathlib import Path
import shutil
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

    def test_republish_and_retirement_invalidate_selected_binding(self):
        old = self.expected(self.bob['profile_id'])
        client = self.remote(old)
        self.result(client, 'inbox')
        self.publish('bob', profile_id=self.bob['profile_id'])
        self.assertTrue(client.call('send', 'bob', '--id', str(uuid.uuid4()), '--message', 'stale')['isError'])
        self.assertEqual(0, self.sql('SELECT count(*) FROM messages')[0][0])
        client.close()
        self.htalk('peer', 'retire', 'bob')
        self.assertEqual('retired', self.export()['profiles'][0]['binding_state'])

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
