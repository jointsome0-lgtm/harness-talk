"""Tailscale source validation for the cross-channel profile catalogue."""
import json
import os
from pathlib import Path
import pwd
import shlex
import shutil
import socket
import subprocess
import sys
import uuid

from compat_support import HtalkCase, wait_for


class TailscaleCatalog(HtalkCase):
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
        self.private_key = self.tmp / 'identity'
        self.known_hosts = self.tmp / 'known_hosts'
        self.private_key.write_text('fixture private key\n')
        self.known_hosts.write_text('fixture pinned host key\n')
        self.private_key.chmod(0o600)
        self.known_hosts.chmod(0o600)

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

    def test_mismatched_pinned_host_key_blocks_catalogue_before_write(self):
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
        endpoint = shlex.join(self.argv(['catalog', 'serve', '--config', str(config)], False))
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
        # This owned fake Tailscale CLI redirects only the fixture peer/port to
        # loopback. OpenSSH still performs a real host-key check and handshake.
        self.ts_binary.write_text('''#!%s -B
import os, select, socket, sys
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
        trust, devices = self.tailscale_trust(['nfixture'])
        device = devices[0]
        for key in ('device_id', 'mailbox_id', 'generation', 'sender'):
            device[key] = directory[key]
        device.update(ssh_port=port, ssh_user=user)
        trust.write_text(json.dumps({'schema_version': 1, 'devices': devices}))
        alias = 'htalk-' + directory['device_id']
        self.known_hosts.write_text(alias + ' ' + Path(str(host) + '.pub').read_text())
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


if __name__ == '__main__':
    import unittest
    unittest.main()
