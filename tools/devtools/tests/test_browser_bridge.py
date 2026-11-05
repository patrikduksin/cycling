"""cycling remote: the launcher's token and paths, and how bridged ports reach esptool and ssh; never opens USB."""

import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from cycling_devtools.device import browser_bridge, flasher, remote


class Launcher(unittest.TestCase):
    def setUp(self):
        self.local = Path(tempfile.mkdtemp())
        bridge_dir = self.local / 'bridge'
        for name, value in {'BRIDGE': bridge_dir, 'TOKEN': bridge_dir / 'token'}.items():
            patcher = patch.object(browser_bridge, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)

    def test_token_file_and_owner_only_session_root(self):
        (self.local / 'bridge').mkdir(mode=0o755)
        (self.local / 'bridge' / 'token').write_text('abc123\n')
        env = browser_bridge.environment({'PATH': '/bin'})
        self.assertEqual(env['CYCLING_REMOTE_TOKEN'], 'abc123')
        self.assertEqual(env['CYCLING_REMOTE_DIR'], str(self.local / 'bridge'))
        self.assertEqual(env['PATH'], '/bin')
        self.assertEqual((self.local / 'bridge').stat().st_mode & 0o777, 0o700)

    def test_environment_token_wins(self):
        (self.local / 'bridge').mkdir()
        (self.local / 'bridge' / 'token').write_text('from-file')
        env = browser_bridge.environment({'CYCLING_REMOTE_TOKEN': 'from-env'})
        self.assertEqual(env['CYCLING_REMOTE_TOKEN'], 'from-env')

    def test_missing_token_names_the_file_and_never_execs(self):
        with patch('os.execve') as execve, self.assertRaises(SystemExit) as stop:
            browser_bridge.main(['7KQM4XP2'])
        self.assertIn(str(self.local / 'bridge' / 'token'), str(stop.exception))
        execve.assert_not_called()

    def test_execs_bun_on_the_agent_with_every_argument(self):
        (self.local / 'bridge').mkdir()
        (self.local / 'bridge' / 'token').write_text('t')
        with patch('shutil.which', return_value='/x/bun'), patch('os.execve') as execve:
            browser_bridge.main(['7KQM4XP2', '--', 'cycling', 'terminal', 'INFO'])
        path, argv, env = execve.call_args.args
        self.assertEqual(path, '/x/bun')
        self.assertEqual(argv, ['/x/bun', str(browser_bridge.AGENT), '7KQM4XP2', '--', 'cycling', 'terminal', 'INFO'])
        self.assertEqual(env['CYCLING_REMOTE_TOKEN'], 't')

    def test_agent_entry_exists(self):
        self.assertTrue(browser_bridge.AGENT.is_file())


class Paths(unittest.TestCase):
    def test_the_bridge_lives_under_local_bridge(self):
        self.assertEqual(browser_bridge.TOKEN, browser_bridge.DEVICE_LOCAL / 'bridge' / 'token')


class EsptoolPort(unittest.TestCase):
    def run_esptool(self, environ):
        completed = subprocess.CompletedProcess([], 0, '', '')
        with (
            tempfile.TemporaryDirectory() as temp,
            patch.dict('os.environ', environ, clear=False),
            patch('subprocess.run', Mock(return_value=completed)) as run,
        ):
            flasher.flasher_for('/dev/pts/9', lambda: '/dev/pts/9', Path(temp)).read(0, 1, Path(temp) / 'out.bin')
        command = run.call_args.args[0]
        return command[command.index('--port') + 1]

    def test_rfc2217_url_replaces_the_bridge_pty_for_esptool(self):
        bridge = {'CYCLING_PORT': '/dev/pts/9', 'CYCLING_ESPTOOL_PORT': 'rfc2217://127.0.0.1:4100'}
        self.assertEqual(self.run_esptool(bridge), 'rfc2217://127.0.0.1:4100')

    def test_another_port_inside_a_bridge_keeps_its_own_path(self):
        bridge = {'CYCLING_PORT': '/dev/pts/4', 'CYCLING_ESPTOOL_PORT': 'rfc2217://127.0.0.1:4100'}
        self.assertEqual(self.run_esptool(bridge), '/dev/pts/9')

    def test_a_stray_esptool_url_without_its_pty_is_ignored(self):
        self.assertEqual(
            self.run_esptool({'CYCLING_PORT': '', 'CYCLING_ESPTOOL_PORT': 'rfc2217://127.0.0.1:1'}), '/dev/pts/9'
        )

    def test_without_a_bridge_esptool_uses_the_selected_port(self):
        self.assertEqual(self.run_esptool({'CYCLING_PORT': '', 'CYCLING_ESPTOOL_PORT': ''}), '/dev/pts/9')


class Ssh(unittest.TestCase):
    def test_bridge_settings_never_cross_to_a_device_host(self):
        environ = {
            'CYCLING_REMOTE_TOKEN': 'secret',
            'CYCLING_REMOTE_DIR': '/x',
            'CYCLING_REMOTE_RELAY': 'ws://localhost',
            'CYCLING_ESPTOOL_PORT': 'rfc2217://127.0.0.1:1',
            'CYCLING_FORWARDED': '1',
        }
        with patch.dict('os.environ', environ, clear=True):
            self.assertEqual(remote.task_environment(), {'CYCLING_FORWARDED': '1'})


if __name__ == '__main__':
    unittest.main()
