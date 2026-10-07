"""Pinned SSH and Tailscale sources for the cross-channel profile catalogue."""
import json
import os
from pathlib import Path
import pwd
import queue
import shlex
import shutil
import socket
import subprocess
import sys
import uuid

from compat_support import HtalkCase, wait_for
from test_mcp import McpClient


class SshCatalogFixture(HtalkCase):
    """A private real SSH listener bound only to loopback, with a forced catalogue."""
    def setUp(self):
        super().setUp()
        self.private_key = self.tmp / 'identity'
        self.known_hosts = self.tmp / 'known_hosts'
        self.private_key.write_text('fixture private key\n')
        self.known_hosts.write_text('fixture pinned host key\n')
        self.private_key.chmod(0o600)
        self.known_hosts.chmod(0o600)

    def ssh_fixture(self, wrapper=None):
        sshd = os.environ.get('HTALK_TEST_SSHD') or shutil.which('sshd')
        keygen = shutil.which('ssh-keygen')
        if not sshd or not keygen:
            self.skipTest('OpenSSH server and ssh-keygen are required for the host-key fixture')
        for peer in ('alice', 'bob'):
            self.htalk('peer', 'add', peer, '--harness', 'generic', '--delivery', 'pull')
        config = self.tmp / 'catalog.json'
        self.htalk('--as', 'alice', 'catalog', 'publish', '--config', config, 'bob',
                   '--name', 'Reviewer', '--role', 'Review')
        directory = self.htalk('catalog', 'export', '--config', config, db=False)
        host = self.tmp / 'host-key'
        wrong = self.tmp / 'wrong-key'
        for path in (host, wrong, self.private_key):
            path.unlink(missing_ok=True)
            subprocess.run([keygen, '-q', '-t', 'ed25519', '-N', '', '-f', str(path)],
                           check=True, capture_output=True)
        with socket.socket() as reservation:
            reservation.bind(('127.0.0.1', 0))
            port = reservation.getsockname()[1]
        endpoint = shlex.join([sys.executable, '-B', str(wrapper)] if wrapper else
                              self.argv(['catalog', 'serve', '--config', str(config)], False))
        authorized = self.tmp / 'authorized_keys'
        authorized.write_text('restrict,command="' + endpoint + '" ' +
                              Path(str(self.private_key) + '.pub').read_text())
        authorized.chmod(0o600)
        user = pwd.getpwuid(os.getuid()).pw_name
        server_config = self.tmp / 'sshd_config'
        server_config.write_text('\n'.join([
            f'Port {port}', 'ListenAddress 127.0.0.1', f'HostKey {host}',
            f'PidFile {self.tmp / "sshd.pid"}', f'AuthorizedKeysFile {authorized}',
            # The owned temporary directory is under /tmp, outside the login
            # user's home. This isolated loopback fixture uses only its own key.
            'StrictModes no', 'PasswordAuthentication no', 'KbdInteractiveAuthentication no',
            'PubkeyAuthentication yes', 'UsePAM no', 'PermitRootLogin prohibit-password',
            f'AllowUsers {user}', 'AllowTcpForwarding no', 'X11Forwarding no',
            'PermitTTY no', 'LogLevel ERROR',
        ]) + '\n')
        subprocess.run([sshd, '-t', '-f', str(server_config)], check=True, capture_output=True)
        log = (self.tmp / 'sshd.log').open('w+')
        self.addCleanup(log.close)
        server = subprocess.Popen([sshd, '-D', '-e', '-f', str(server_config)], stderr=log)
        self.processes.append(server)
        def listening():
            if server.poll() is not None:
                log.seek(0)
                self.fail('fixture sshd stopped: ' + log.read())
            try:
                with socket.create_connection(('127.0.0.1', port), timeout=0.2):
                    return True
            except OSError:
                return False
        wait_for(listening, message='fixture SSH listener')
        device = {key: directory[key] for key in ('device_id', 'mailbox_id', 'generation', 'sender')}
        device.update(ssh_port=port, ssh_user=user, identity_file=str(self.private_key),
                      known_hosts_file=str(self.known_hosts))
        trust = self.tmp / 'ssh-trust.json'
        trust.write_text(json.dumps({'schema_version': 1, 'devices': [device]}))
        trust.chmod(0o600)
        alias = 'htalk-' + directory['device_id']
        self.known_hosts.write_text(alias + ' ' + Path(str(host) + '.pub').read_text())
        return config, trust, directory, wrong, log


class TailscaleCatalog(SshCatalogFixture):
    def setUp(self):
        super().setUp()
        self.ts_binary = self.tmp / 'tailscale'
        self.ts_namespace = self.tmp / 'tailscale-control'
        self.ts_namespace.mkdir(mode=0o700)
        self.ts_socket = self.ts_namespace / 'tailscale.sock'
        self.status_file = self.tmp / 'status.json'
        self.calls_file = self.tmp / 'tailscale-calls.jsonl'
        self.ts_binary.write_text(
            '#!%s -B\n'
            'import json, os, sys\n'
            'with open(os.environ["HTALK_FAKE_TAILSCALE_CALLS"], "a") as f:\n'
            '    f.write(json.dumps(sys.argv[1:]) + "\\n")\n'
            'if sys.argv[1:] == ["--socket=" + os.environ["HTALK_FAKE_TAILSCALE_SOCKET"], "status", "--json"]:\n'
            '    sys.stdout.write(open(os.environ["HTALK_FAKE_TAILSCALE_STATUS"]).read())\n'
            '    sys.exit(int(os.environ.get("HTALK_FAKE_TAILSCALE_EXIT", "0")))\n'
            'else:\n'
            '    sys.exit(91)\n' % sys.executable
        )
        self.ts_binary.chmod(0o755)
        # A bound Unix socket is enough for the local ownership/type check. The
        # fake CLI prints fixtures and never connects to the socket.
        endpoint = socket.socket(socket.AF_UNIX)
        endpoint.bind(str(self.ts_socket))
        endpoint.close()
        self.ts_socket.chmod(0o666)
        self.addCleanup(lambda: self.ts_socket.unlink(missing_ok=True))

    def tailscale_trust(self, peers):
        devices = []
        for peer_id in peers:
            devices.append({
                'device_id': str(uuid.uuid4()),
                'mailbox_id': str(uuid.uuid4()),
                'generation': str(uuid.uuid4()),
                'sender': 'alice',
                'ssh_user': 'catalog',
                'ssh_port': 22,
                'identity_file': str(self.private_key),
                'known_hosts_file': str(self.known_hosts),
                'tailscale_peer_id': peer_id,
            })
        path = self.tmp / 'tailscale-trust.json'
        path.write_text(json.dumps({'schema_version': 1, 'devices': devices}))
        path.chmod(0o600)
        return path, devices

    def status(self, peers, *, backend='Running'):
        value = {
            'Version': '1.102.4',
            'BackendState': backend,
            'Self': {'ID': 'nlocal', 'Online': True, 'TailscaleIPs': ['100.64.0.1']},
            'Peer': peers,
        }
        self.status_file.write_text(json.dumps(value))

    def env(self, extra=None):
        value = {
            'HTALK_FAKE_TAILSCALE_CALLS': str(self.calls_file),
            'HTALK_FAKE_TAILSCALE_SOCKET': str(self.ts_socket),
            'HTALK_FAKE_TAILSCALE_STATUS': str(self.status_file),
        }
        value.update(extra or {})
        return value

    def environment(self, extra=None):
        return super().environment(self.env(extra))

    def discover(self, trust):
        return self.htalk(
            'catalog', 'discover', '--trust', trust, '--via', 'tailscale',
            '--tailscale-binary', self.ts_binary, '--tailscale-socket', self.ts_socket,
            '--seconds', '1', db=False, env=self.env(),
        )

    def ts_calls(self):
        if not self.calls_file.exists():
            return []
        return [json.loads(line) for line in self.calls_file.read_text().splitlines()]

    def ssh_fixture(self):
        config, trust, directory, wrong, log = super().ssh_fixture()
        port = json.loads(trust.read_text())['devices'][0]['ssh_port']
        # This owned fake Tailscale CLI redirects only the fixture peer/port to
        # loopback. OpenSSH still performs a real host-key check and handshake.
        self.ts_binary.write_text('''#!%s -B
import json, os, select, socket, sys
with open(os.environ['HTALK_FAKE_TAILSCALE_CALLS'], 'a') as out:
    out.write(json.dumps(sys.argv[1:]) + '\\n')
if sys.argv[2:] == ['status', '--json']:
    sys.stdout.write(open(os.environ['HTALK_FAKE_TAILSCALE_STATUS']).read())
elif sys.argv[2:] == ['nc', '100.64.0.2', '%s']:
    with socket.create_connection(('127.0.0.1', %s)) as link:
        while True:
            ready, _, _ = select.select([0, link], [], [])
            if 0 in ready:
                data = os.read(0, 65536)
                if not data:
                    break
                link.sendall(data)
            if link in ready:
                data = link.recv(65536)
                if not data:
                    break
                os.write(1, data)
else:
    sys.exit(91)
''' % (sys.executable, port, port))
        self.status({'nodekey:fixture': {
            'ID': 'nfixture', 'Online': True, 'TailscaleIPs': ['100.64.0.2'],
        }})
        value = json.loads(trust.read_text())
        value['devices'][0]['tailscale_peer_id'] = 'nfixture'
        trust.write_text(json.dumps(value))
        return config, trust, directory, wrong, log

    def connected(self, trust, selector):
        # All discovery, route checks and SSH calls use this fixture's files and
        # loopback listener; no installed Tailscale daemon or remote host is used.
        try:
            return McpClient(self, argv=self.argv([
                'catalog', 'connect', selector, '--trust', trust, '--via', 'tailscale',
                '--tailscale-binary', self.ts_binary, '--tailscale-socket', self.ts_socket,
                '--seconds', '1',
            ], False))
        except queue.Empty:
            process = self.processes[-1]
            if process.poll() is not None:
                self.fail('catalog connect failed before MCP initialization: ' + process.stderr.read())
            raise

    def test_mismatched_pinned_host_key_blocks_catalogue_before_write(self):
        _, trust, directory, wrong, log = self.ssh_fixture()
        alias = 'htalk-' + directory['device_id']
        before = self.db.read_bytes()
        good = self.discover(trust)
        log.seek(0)
        self.assertEqual('reachable', good['devices'][0]['directory_state'], (good, log.read()))
        self.assertEqual(directory, good['devices'][0]['catalog'])
        self.known_hosts.write_text(alias + ' ' + Path(str(wrong) + '.pub').read_text())
        rejected = self.discover(trust)
        self.assertEqual('unavailable', rejected['devices'][0]['directory_state'])
        self.assertEqual('catalog_unreachable', rejected['devices'][0]['error'])
        self.assertEqual('partial', rejected['sources'][0]['status'])
        blocked = self.run_raw('catalog', 'connect', 'Reviewer', '--trust', trust, '--via', 'tailscale',
                               '--tailscale-binary', self.ts_binary, '--tailscale-socket', self.ts_socket,
                               '--seconds', '1', db=False, env=self.env())
        self.assertEqual(2, blocked.code)
        self.assertIn('catalog_profile_unavailable', blocked.stderr)
        self.assertEqual('', blocked.stdout)
        self.assertEqual(before, self.db.read_bytes())
        self.assertEqual(0, self.sql('SELECT count(*) FROM messages')[0][0])

    def test_changed_channel_binding_blocks_open_client_before_connector(self):
        _, trust, _, _, _ = self.ssh_fixture()
        client = self.connected(trust, 'Reviewer')
        healthy = {'ID': 'nfixture', 'Online': True, 'TailscaleIPs': ['100.64.0.2']}
        self.assertFalse(client.call('inbox').get('isError', False))
        for label, peer, local_id in (
            ('selected peer changed', {**healthy, 'ID': 'nreplacement'}, 'nlocal'),
            ('selected peer address changed', {**healthy, 'TailscaleIPs': ['100.64.0.3']}, 'nlocal'),
            ('local daemon identity changed', healthy, 'nreplacement'),
            ('selected peer went offline', {**healthy, 'Online': False}, 'nlocal'),
        ):
            with self.subTest(label=label):
                self.status({'nodekey:fixture': peer})
                status = json.loads(self.status_file.read_text())
                status['Self']['ID'] = local_id
                self.status_file.write_text(json.dumps(status))
                connectors_before = [call for call in self.ts_calls() if 'nc' in call]
                blocked = client.call('send', 'bob', '--id', str(uuid.uuid4()), '--message', label)
                self.assertTrue(blocked['isError'])
                self.assertIn('selected channel binding', json.dumps(blocked))
                self.assertIn('no mailbox command was sent', json.dumps(blocked))
                self.assertEqual(0, self.sql('SELECT count(*) FROM messages')[0][0])
                self.assertEqual(connectors_before, [call for call in self.ts_calls() if 'nc' in call])
                self.status({'nodekey:fixture': healthy})
                self.assertFalse(client.call('inbox').get('isError', False))
        client.close()

    def test_uuid_selector_selects_identity_and_shadow_name_uses_own_uuid(self):
        config, trust, directory, _, _ = self.ssh_fixture()
        profile_id = directory['profiles'][0]['profile_id']
        self.htalk('peer', 'add', 'carol', '--harness', 'generic', '--delivery', 'pull')
        shadow = self.htalk('--as', 'alice', 'catalog', 'publish', '--config', config,
                            'carol', '--name', profile_id, '--role', 'Review')
        for selector, expected_peer in ((profile_id, 'bob'), (shadow['profile_id'], 'carol')):
            client = self.connected(trust, selector)
            result = client.call('peer', 'list')
            self.assertFalse(result.get('isError', False), result)
            self.assertEqual([expected_peer], [p['name'] for p in result['structuredContent']['result']['peers']])
            client.close()
        self.assertEqual(0, self.sql('SELECT count(*) FROM messages')[0][0])

    def test_binary_and_socket_paths_with_a_quote_a_percent_token_and_a_space_still_connect(self):
        _, trust, _, _, _ = self.ssh_fixture()
        port = json.loads(trust.read_text())['devices'][0]['ssh_port']
        # OpenSSH expands percent tokens in the proxy command and then hands it to a shell.
        odd = self.tmp / "own a'b%h"
        odd.mkdir(mode=0o700)
        self.ts_binary = self.ts_binary.rename(odd / "tail'scale %h")
        self.ts_socket = odd / "tail'scale %h.sock"
        endpoint = socket.socket(socket.AF_UNIX)
        endpoint.bind(str(self.ts_socket))
        endpoint.close()
        client = self.connected(trust, 'Reviewer')
        result = client.call('peer', 'list')
        self.assertFalse(result.get('isError', False), result)
        self.assertEqual(['bob'], [p['name'] for p in result['structuredContent']['result']['peers']])
        client.close()
        self.assertIn(['--socket=' + str(self.ts_socket), 'nc', '100.64.0.2', str(port)], self.ts_calls())

    def test_missing_uuid_selector_never_falls_back_to_display_name(self):
        config, trust, _, _, _ = self.ssh_fixture()
        missing = str(uuid.uuid4())
        self.htalk('--as', 'alice', 'catalog', 'publish', '--config', config,
                   'bob', '--name', missing, '--role', 'Review')
        before = self.db.read_bytes()
        initialize = json.dumps({'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {
            'protocolVersion': '2025-06-18', 'capabilities': {},
            'clientInfo': {'name': 'fixture', 'version': '1'},
        }}) + '\n' + json.dumps({'jsonrpc': '2.0', 'method': 'notifications/initialized'}) + '\n'
        result = subprocess.run(self.argv([
            'catalog', 'connect', missing, '--trust', trust, '--via', 'tailscale',
            '--tailscale-binary', self.ts_binary, '--tailscale-socket', self.ts_socket,
            '--seconds', '1',
        ], False), input=initialize, capture_output=True, text=True, timeout=15,
            env=self.environment(self.env()), cwd=self.tmp)
        self.assertEqual(2, result.returncode, result.stdout + result.stderr)
        self.assertIn('catalog_profile_unavailable', result.stderr)
        self.assertEqual('', result.stdout)
        self.assertEqual(before, self.db.read_bytes())
        self.assertEqual(0, self.sql('SELECT count(*) FROM messages')[0][0])

    def assert_tailscale_source_error(self, trust, expected_error, env=None):
        result = self.run_raw(
            'catalog', 'discover', '--trust', trust, '--via', 'tailscale',
            '--tailscale-binary', self.ts_binary, '--tailscale-socket', self.ts_socket,
            '--seconds', '1', db=False, env=self.env(env),
        )
        self.assertEqual(2, result.code, result.stdout + result.stderr)
        self.assertEqual('error', result.json.get('state'), result.json)
        self.assertEqual(expected_error, result.json.get('error'), result.json)

    def test_unknown_and_offline_peers_are_not_fetched(self):
        trust, devices = self.tailscale_trust(['nknown-offline', 'nknown-no-address'])
        self.status({
            'nodekey:offline': {
                'ID': 'nknown-offline', 'Online': False, 'TailscaleIPs': ['100.64.0.2'],
            },
            'nodekey:no-address': {
                'ID': 'nknown-no-address', 'Online': True, 'TailscaleIPs': [],
            },
            'nodekey:untrusted': {
                'ID': 'nnot-trusted', 'Online': True, 'TailscaleIPs': ['100.64.0.9'],
            },
        })

        result = self.discover(trust)

        by_id = {item['device_id']: item for item in result['devices']}
        for device in devices:
            self.assertEqual('unavailable', by_id[device['device_id']]['directory_state'])
            self.assertEqual('catalog_tailscale_peer_unavailable', by_id[device['device_id']]['error'])
        self.assertEqual({item['device_id'] for item in devices}, set(by_id))
        source = next(item for item in result['sources'] if item['source'] == 'tailscale')
        self.assertEqual('partial', source['status'])
        self.assertEqual(1, source['unknown_peer_count'])
        self.assertEqual(
            [['--socket=' + str(self.ts_socket), 'status', '--json']],
            self.ts_calls(),
            'unknown, offline, and addressless peers must not start an endpoint call',
        )

    def test_permissive_socket_inside_protected_namespace_is_accepted(self):
        trust, _ = self.tailscale_trust([])
        self.status({})

        result = self.discover(trust)

        self.assertEqual([], result['devices'])
        source = next(item for item in result['sources'] if item['source'] == 'tailscale')
        self.assertEqual('ok', source['status'])
        self.assertEqual(0, source['unknown_peer_count'])
        self.assertEqual(0o666, self.ts_socket.stat().st_mode & 0o777)
        self.assertEqual(0o700, self.ts_namespace.stat().st_mode & 0o777)
        self.assertEqual(
            [['--socket=' + str(self.ts_socket), 'status', '--json']],
            self.ts_calls(),
        )

    def test_malformed_status_stopped_backend_and_duplicate_ids_are_source_errors(self):
        trust, _ = self.tailscale_trust(['nknown'])
        cases = (
            ('malformed JSON', '{not-json'),
            ('backend not running', json.dumps({
                'Version': '1.102.4',
                'BackendState': 'Stopped',
                'Self': {'ID': 'nlocal', 'Online': True, 'TailscaleIPs': ['100.64.0.1']},
                'Peer': {},
            })),
            ('duplicate peer IDs', json.dumps({
                'Version': '1.102.4',
                'BackendState': 'Running',
                'Self': {'ID': 'nlocal', 'Online': True, 'TailscaleIPs': ['100.64.0.1']},
                'Peer': {
                    'nodekey:first': {'ID': 'nknown', 'Online': True, 'TailscaleIPs': ['100.64.0.2']},
                    'nodekey:second': {'ID': 'nknown', 'Online': True, 'TailscaleIPs': ['100.64.0.3']},
                },
            })),
        )
        expected_errors = {
            'malformed JSON': 'catalog_tailscale_invalid_status',
            'backend not running': 'catalog_tailscale_unavailable',
            'duplicate peer IDs': 'catalog_tailscale_invalid_status',
        }
        for label, fixture in cases:
            with self.subTest(label=label):
                self.status_file.write_text(fixture)
                self.calls_file.unlink(missing_ok=True)
                self.assert_tailscale_source_error(trust, expected_errors[label])

        for label, local, online, address in (
                ('address outside the tailnet', 'nlocal', True, '192.168.1.2'),
                ('online is not a boolean', 'nlocal', None, '100.64.0.2'),
                ('peer has the local ID', 'nknown', True, '100.64.0.2')):
            with self.subTest(label=label):
                self.status_file.write_text(json.dumps({
                    'Version': '1.102.4', 'BackendState': 'Running',
                    'Self': {'ID': local, 'Online': True, 'TailscaleIPs': ['100.64.0.1']},
                    'Peer': {'nodekey:first': {'ID': 'nknown', 'Online': online, 'TailscaleIPs': [address]}},
                }))
                self.calls_file.unlink(missing_ok=True)
                self.assert_tailscale_source_error(trust, 'catalog_tailscale_invalid_status')

        with self.subTest(label='tailscale CLI failure'):
            self.status({})
            self.calls_file.unlink(missing_ok=True)
            self.assert_tailscale_source_error(
                trust, 'catalog_tailscale_unavailable', {'HTALK_FAKE_TAILSCALE_EXIT': '1'},
            )

        with self.subTest(label='unsupported status version'):
            self.status_file.write_text(json.dumps({
                'Version': '1.103.0', 'BackendState': 'Running',
                'Self': {'ID': 'nlocal', 'Online': True, 'TailscaleIPs': ['100.64.0.1']},
                'Peer': {},
            }))
            self.calls_file.unlink(missing_ok=True)
            self.assert_tailscale_source_error(trust, 'catalog_tailscale_unsupported_version')

    def test_invalid_tailscale_peer_id_is_rejected_before_status_call(self):
        trust, _ = self.tailscale_trust(['bad peer id'])
        self.status({})

        result = self.run_raw(
            'catalog', 'discover', '--trust', trust, '--via', 'tailscale',
            '--tailscale-binary', self.ts_binary, '--tailscale-socket', self.ts_socket,
            '--seconds', '1', db=False, env=self.env(),
        )

        self.assertEqual(2, result.code, result.stdout + result.stderr)
        self.assertEqual('error', result.json.get('state'), result.json)
        self.assertEqual('catalog_invalid_trust', result.json.get('error'))
        self.assertEqual([], self.ts_calls())

    def test_world_writable_tailscale_binary_is_rejected_before_status_call(self):
        trust, _ = self.tailscale_trust(['nknown'])
        self.status({})
        self.ts_binary.chmod(0o777)

        result = self.run_raw(
            'catalog', 'discover', '--trust', trust, '--via', 'tailscale',
            '--tailscale-binary', self.ts_binary, '--tailscale-socket', self.ts_socket,
            '--seconds', '1', db=False, env=self.env(),
        )

        self.assertEqual(2, result.code, result.stdout + result.stderr)
        self.assertEqual('error', result.json.get('state'), result.json)
        self.assertEqual('catalog_tailscale_invalid_binary', result.json.get('error'))
        self.assertEqual([], self.ts_calls())

    def test_group_or_world_writable_socket_namespace_is_rejected_before_status_call(self):
        trust, _ = self.tailscale_trust(['nknown'])
        self.status({})
        for mode in (0o770, 0o777):
            with self.subTest(mode=oct(mode)):
                self.ts_namespace.chmod(mode)
                result = self.run_raw(
                    'catalog', 'discover', '--trust', trust, '--via', 'tailscale',
                    '--tailscale-binary', self.ts_binary, '--tailscale-socket', self.ts_socket,
                    '--seconds', '1', db=False, env=self.env(),
                )

                self.assertEqual(2, result.code, result.stdout + result.stderr)
                self.assertEqual('error', result.json.get('state'), result.json)
                self.assertEqual('catalog_tailscale_invalid_socket', result.json.get('error'))
                self.assertEqual([], self.ts_calls())
                self.ts_namespace.chmod(0o700)


class DirectSshCatalog(SshCatalogFixture):
    def pin_address(self, trust, address='127.0.0.1'):
        value = json.loads(trust.read_text())
        value['devices'][0]['ssh_address'] = address
        trust.write_text(json.dumps(value))

    def discover(self, trust):
        return self.htalk('catalog', 'discover', '--trust', trust, '--via', 'ssh', db=False)

    def connect_args(self, trust, selector):
        return ['catalog', 'connect', selector, '--trust', str(trust), '--via', 'ssh']

    def connected(self, trust, selector='Reviewer'):
        return McpClient(self, argv=self.argv(self.connect_args(trust, selector), False))

    def connect_error(self, trust, selector, error):
        # connect reserves stdout for MCP frames; startup errors use stderr.
        result = self.run_raw(*self.connect_args(trust, selector), db=False)
        self.assertEqual(2, result.code, result.stderr)
        self.assertEqual('', result.stdout)
        self.assertIn(error, result.stderr)

    def result(self, client, *args):
        result = client.call(*args)
        self.assertFalse(result.get('isError', False), result)
        return result['structuredContent']['result']

    def test_missing_address_keeps_legacy_trust_valid_and_reports_partial(self):
        _, trust, directory, _, _ = self.ssh_fixture()
        before = self.db.read_bytes()
        result = self.discover(trust)
        self.assertEqual('partial', result['sources'][0]['status'])
        self.assertEqual('catalog_ssh_not_bound', result['devices'][0]['error'])
        self.pin_address(trust)
        result = self.discover(trust)
        self.assertEqual('ok', result['sources'][0]['status'])
        self.assertEqual('ssh', result['sources'][0]['source'])
        self.assertEqual(directory, result['devices'][0]['catalog'])
        self.assertEqual(before, self.db.read_bytes())
        # No fake or installed Tailscale daemon and no multicast interface are
        # configured in this fixture. Only its pinned loopback SSH endpoint runs.

    def test_malformed_addresses_are_rejected_before_any_endpoint_or_database(self):
        device = {key: str(uuid.uuid4()) for key in ('device_id', 'mailbox_id', 'generation')}
        device.update(sender='alice', ssh_user='fixture', identity_file=str(self.private_key),
                      known_hosts_file=str(self.known_hosts))
        trust = self.tmp / 'trust.json'
        for address in ('', 'example.com', '::1', '127.0.0.1:22', ' 127.0.0.1',
                        '127.0.0.01', '0.0.0.0', '224.0.0.1', '255.255.255.255',
                        '-oProxyCommand=command'):
            with self.subTest(address=address):
                device['ssh_address'] = address
                trust.write_text(json.dumps({'schema_version': 1, 'devices': [device]}))
                trust.chmod(0o600)
                self.error('catalog', 'discover', '--trust', trust, '--via', 'ssh', db=False,
                           error='catalog_invalid_ssh_address')
                self.assertFalse(self.db.exists())

    def test_partial_snapshot_contains_only_configured_devices(self):
        _, trust, directory, _, _ = self.ssh_fixture()
        self.pin_address(trust)
        value = json.loads(trust.read_text())
        unreachable = {**value['devices'][0], 'device_id': str(uuid.uuid4()),
                       'ssh_address': '127.0.0.2'}
        unbound = {**value['devices'][0], 'device_id': str(uuid.uuid4())}
        del unbound['ssh_address']
        value['devices'] += [unreachable, unbound]
        trust.write_text(json.dumps(value))
        result = self.discover(trust)
        by_id = {item['device_id']: item for item in result['devices']}
        self.assertEqual({d['device_id'] for d in value['devices']}, set(by_id))
        self.assertEqual('reachable', by_id[directory['device_id']]['directory_state'])
        self.assertEqual('catalog_unreachable', by_id[unreachable['device_id']]['error'])
        self.assertEqual('catalog_ssh_not_bound', by_id[unbound['device_id']]['error'])
        self.assertEqual('partial', result['sources'][0]['status'])
        self.error('catalog', 'discover', '--trust', trust, '--via', 'ssh', '--interface', 'lo',
                   db=False, error='catalog_ssh_omit_interface')
        self.assertEqual(0, self.sql('SELECT count(*) FROM messages')[0][0])

    def test_name_uuid_shadow_and_ambiguity_keep_existing_selection_rules(self):
        config, trust, directory, _, _ = self.ssh_fixture()
        self.pin_address(trust)
        profile_id = directory['profiles'][0]['profile_id']
        self.htalk('peer', 'add', 'carol', '--harness', 'generic', '--delivery', 'pull')
        shadow = self.htalk('--as', 'alice', 'catalog', 'publish', '--config', config,
                            'carol', '--name', profile_id, '--role', 'Review')
        for selector, peer in (('Reviewer', 'bob'), (profile_id, 'bob'),
                               (shadow['profile_id'], 'carol')):
            with self.subTest(selector=selector):
                client = self.connected(trust, selector)
                self.assertEqual([peer], [p['name'] for p in self.result(client, 'peer', 'list')['peers']])
                client.close()
        self.htalk('--as', 'alice', 'catalog', 'publish', '--config', config,
                   'carol', '--profile-id', shadow['profile_id'], '--name', 'Reviewer', '--role', 'Review')
        self.connect_error(trust, 'Reviewer', 'catalog_ambiguous_profile')
        missing = str(uuid.uuid4())
        self.htalk('--as', 'alice', 'catalog', 'publish', '--config', config,
                   'carol', '--profile-id', shadow['profile_id'], '--name', missing, '--role', 'Review')
        self.connect_error(trust, missing, 'catalog_profile_unavailable')
        for selector in ('reviewer', profile_id.upper()):
            self.connect_error(trust, selector, 'catalog_profile_unavailable')
        self.htalk('--as', 'alice', 'catalog', 'publish', '--config', config,
                   'carol', '--profile-id', shadow['profile_id'], '--name', profile_id.upper(), '--role', 'Review')
        client = self.connected(trust, profile_id.upper())
        self.assertEqual(['carol'], [p['name'] for p in self.result(client, 'peer', 'list')['peers']])
        client.close()
        self.htalk('--as', 'alice', 'catalog', 'publish', '--config', config,
                   'carol', '--profile-id', shadow['profile_id'], '--name', profile_id, '--role', 'Review')
        self.htalk('peer', 'retire', 'bob')
        self.connect_error(trust, profile_id, 'catalog_profile_unavailable')
        self.assertEqual(0, self.sql('SELECT count(*) FROM messages')[0][0])

    def test_one_profile_identity_on_two_devices_is_refused(self):
        _, trust, directory, _, _ = self.ssh_fixture()
        self.pin_address(trust)
        profile_id = directory['profiles'][0]['profile_id']
        # A second device on the same listener: its own catalogue, key and forced command.
        config = self.tmp / 'second.json'
        self.htalk('--as', 'alice', 'catalog', 'publish', '--config', config, 'bob',
                   '--profile-id', profile_id, '--name', 'Reviewer', '--role', 'Review')
        second = self.htalk('catalog', 'export', '--config', config, db=False)
        key = self.tmp / 'second-identity'
        subprocess.run([shutil.which('ssh-keygen'), '-q', '-t', 'ed25519', '-N', '', '-f', str(key)],
                       check=True, capture_output=True)
        endpoint = shlex.join(self.argv(['catalog', 'serve', '--config', str(config)], False))
        with (self.tmp / 'authorized_keys').open('a') as authorized:
            authorized.write('restrict,command="' + endpoint + '" ' + Path(str(key) + '.pub').read_text())
        with self.known_hosts.open('a') as known:
            known.write('htalk-' + second['device_id'] + ' ' + (self.tmp / 'host-key.pub').read_text())
        value = json.loads(trust.read_text())
        value['devices'].append({**value['devices'][0], 'identity_file': str(key),
                                 **{key: second[key] for key in ('device_id', 'mailbox_id', 'generation', 'sender')}})
        trust.write_text(json.dumps(value))
        found = self.discover(trust)
        self.assertEqual(['reachable', 'reachable'], [d['directory_state'] for d in found['devices']])
        for selector in (profile_id, 'Reviewer'):
            self.connect_error(trust, selector, 'catalog_ambiguous_profile')

    def test_wrong_host_key_and_deployment_block_discovery_before_write(self):
        _, trust, directory, wrong, _ = self.ssh_fixture()
        self.pin_address(trust)
        alias = 'htalk-' + directory['device_id']
        correct_key = self.known_hosts.read_text()
        before = self.db.read_bytes()
        self.known_hosts.write_text(alias + ' ' + Path(str(wrong) + '.pub').read_text())
        rejected = self.discover(trust)
        self.assertEqual('catalog_unreachable', rejected['devices'][0]['error'])
        self.connect_error(trust, 'Reviewer', 'catalog_profile_unavailable')
        self.known_hosts.write_text(correct_key)
        value = json.loads(trust.read_text())
        for key in ('mailbox_id', 'generation', 'sender'):
            with self.subTest(binding=key):
                bad = json.loads(json.dumps(value))
                bad['devices'][0][key] = 'other' if key == 'sender' else str(uuid.uuid4())
                trust.write_text(json.dumps(bad))
                rejected = self.discover(trust)
                self.assertEqual('catalog_endpoint_binding_mismatch', rejected['devices'][0]['error'])
                self.connect_error(trust, 'Reviewer', 'catalog_profile_unavailable')
        self.assertEqual(before, self.db.read_bytes())

    def test_republication_and_retirement_block_open_client_before_write(self):
        config, trust, directory, _, _ = self.ssh_fixture()
        self.pin_address(trust)
        profile_id = directory['profiles'][0]['profile_id']
        client = self.connected(trust, profile_id)
        self.result(client, 'inbox')
        self.htalk('--as', 'alice', 'catalog', 'publish', '--config', config, 'bob',
                   '--profile-id', profile_id, '--name', 'Reviewer', '--role', 'Review')
        blocked = client.call('send', 'bob', '--id', str(uuid.uuid4()), '--message', 'stale')
        self.assertTrue(blocked['isError'])
        self.assertIn('before sending', json.dumps(blocked))
        client.close()
        fresh = self.connected(trust, profile_id)
        self.htalk('peer', 'retire', 'bob')
        blocked = fresh.call('send', 'bob', '--id', str(uuid.uuid4()), '--message', 'retired')
        self.assertTrue(blocked['isError'])
        self.assertIn('before sending', json.dumps(blocked))
        fresh.close()
        self.assertEqual(0, self.sql('SELECT count(*) FROM messages')[0][0])

    def test_open_client_keeps_address_until_explicit_rediscovery(self):
        _, trust, _, _, _ = self.ssh_fixture()
        self.pin_address(trust)
        client = self.connected(trust)
        self.pin_address(trust, '127.0.0.2')
        self.assertEqual('unavailable', self.discover(trust)['devices'][0]['directory_state'])
        self.connect_error(trust, 'Reviewer', 'catalog_profile_unavailable')
        self.result(client, 'inbox')  # Still the originally captured 127.0.0.1.
        self.pin_address(trust)
        fresh = self.connected(trust)
        self.result(fresh, 'inbox')
        fresh.close()
        client.close()
        self.assertEqual(0, self.sql('SELECT count(*) FROM messages')[0][0])

    def test_lost_ssh_write_response_keeps_one_attempt_and_original_ids(self):
        mode = self.tmp / 'link-mode'
        mode.write_text('drop')
        attempts = self.tmp / 'attempts'
        wrapper = self.tmp / 'drop-response.py'
        command = self.argv(['catalog', 'serve', '--config', str(self.tmp / 'catalog.json')], False)
        wrapper.write_text('''import json,os,subprocess,sys
from pathlib import Path
command = %r
if os.environ.get('SSH_ORIGINAL_COMMAND') != 'mcp' or Path(%r).read_text() == 'online':
    os.execv(command[0],command)
child = subprocess.Popen(command,stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
try:
    for line in sys.stdin:
        frame=json.loads(line)
        child.stdin.write(line);child.stdin.flush()
        if 'id' not in frame:
            continue
        response=child.stdout.readline()
        if frame['method']=='tools/call':
            with open(%r,'a') as out:
                out.write(json.dumps(frame['params'])+'\\n')
            assert not json.loads(response)['result'].get('isError',False)
            break
        sys.stdout.write(response);sys.stdout.flush()
finally:
    child.stdin.close();child.wait(timeout=10)
''' % (command, str(mode), str(attempts)))
        _, trust, _, _, _ = self.ssh_fixture(wrapper=wrapper)
        self.pin_address(trust)
        client = self.connected(trust)
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
        conflict = client.call('send', 'bob', '--id', request_id, '--message', 'changed')
        self.assertTrue(conflict['isError'])
        self.assertIn('message_id_conflict', json.dumps(conflict))
        parent = self.htalk('--as', 'bob', 'send', 'alice', '--id', str(uuid.uuid4()),
                            '--message', 'incoming request')
        mode.write_text('drop')
        lost = client.call('reply', parent['id'], '--message', 'one answer')
        self.assertTrue(lost['isError'])
        self.assertIn('outcome is unknown', json.dumps(lost))
        self.assertEqual(2, len(attempts.read_text().splitlines()))
        self.assertEqual(3, self.sql('SELECT count(*) FROM messages')[0][0])
        mode.write_text('online')
        answer = self.result(client, 'show', parent['id'])['reply']
        retry = self.result(client, 'reply', parent['id'], '--message', 'one answer')
        self.assertEqual((answer['id'], answer['created_at']), (retry['id'], retry['created_at']))
        conflict = client.call('reply', parent['id'], '--message', 'changed answer')
        self.assertTrue(conflict['isError'])
        self.assertIn('reply_conflict_existing_answer_preserved', json.dumps(conflict))
        self.assertEqual('one answer', self.result(client, 'show', parent['id'])['reply']['body'])
        self.assertEqual(3, self.sql('SELECT count(*) FROM messages')[0][0])
        self.assertEqual(2, len(attempts.read_text().splitlines()))
        client.close()


if __name__ == '__main__':
    import unittest
    unittest.main()
