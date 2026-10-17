import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import cycling_devtools.harness.runner as harness
from cycling_devtools.harness.connectivity import scenarios
from workspace_fixture import private_root


class ControlTransport:
    request_id = 0

    def __init__(self):
        self.sequence = 0
        self.pending_reads = 0
        self.sent = []

    def terminal_command(self, command, **_):
        self.request_id += 1
        self.sent.append(command)
        if command == 'WIFI':
            operation = 'pending' if self.pending_reads else ('completed' if self.sequence else 'idle')
            self.pending_reads = max(0, self.pending_reads - 1)
            return {'status': 'OK', 'data': f'sequence={self.sequence} operation={operation} saved=true error=none'}
        if command.startswith('WIFI CONFIG '):
            self.sequence += 1
            self.pending_reads = 1
            return {'status': 'ACCEPTED', 'data': ''}
        raise AssertionError('unexpected transport command')

    def delay(self, seconds):
        pass


class ConnectivityTests(unittest.TestCase):
    def test_runtime_operations_require_original_restoration_plan(self):
        with self.assertRaises(ValueError):
            harness.preflight({'steps': [{'op': 'wifi', 'action': 'scan'}]})
        with self.assertRaises(ValueError):
            harness.preflight(
                {
                    'connectivity_restore': {'wifi': {'profile': None, 'connected': False}},
                    'steps': [{'op': 'wifi', 'action': 'raw'}],
                }
            )
        harness.preflight(
            {
                'connectivity_restore': {'wifi': {'profile': None, 'connected': False}},
                'steps': [{'op': 'wifi', 'action': 'scan'}],
            }
        )

    def test_accepted_configuration_waits_for_its_sequence_and_trace_redacts_credentials(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            local = root / '.local'
            local.mkdir()
            profile = local / 'profile.json'
            config = {'ssid': 'Owned test', 'password': 'private-test-password', 'security': 'wpa-psk'}
            profile.write_text(json.dumps(config))
            with private_root(root):
                transport = ControlTransport()
                runner = harness.Runner(transport, local)
                try:
                    result = runner.connectivity('wifi', {'action': 'configure', 'profile': str(profile)})
                finally:
                    runner.trace.close()
                self.assertEqual(result['operation'], 'completed')
                self.assertEqual(result['sequence'], '1')
                self.assertEqual(transport.sent.count('WIFI'), 3)
                trace = (local / 'commands.jsonl').read_text()
                self.assertNotIn(config['password'], trace)
                self.assertNotIn(config['password'].encode().hex(), trace)
                self.assertIn('<private configuration>', trace)

    def test_generated_failure_and_recovery_steps_reuse_original_profile(self):
        generated = scenarios(Path('/private/profile.json'))
        wifi = generated['connectivity-wifi']
        failures = [s for s in wifi['steps'] if s.get('completion') == 'error']
        self.assertEqual(
            failures, [{'op': 'wifi', 'action': 'connect', 'completion': 'error', 'expect': {'online': 'false'}}]
        )
        self.assertEqual(wifi['connectivity_restore']['wifi']['profile'], '/private/profile.json')
        ble = generated['connectivity-ble']
        self.assertEqual(
            ble['connectivity_restore']['ble'], {'mode': 'authorized-heart', 'connected': False, 'echo_after': True}
        )

    def test_ble_scenario_allows_gatt_deadlines_and_checks_reconnected_measurements(self):
        ble = scenarios(Path('/private/profile.json'))['connectivity-ble']
        connects = [s for s in ble['steps'] if s.get('action') == 'connect']
        self.assertEqual(len(connects), 3)
        self.assertTrue(all(s['timeout_s'] >= 77 for s in connects))
        heart_checks = [
            s
            for s in ble['steps']
            if s.get('command') == 'RIDE SENSORS' and s.get('equals', {}).get('heart') == 'Some(72)'
        ]
        self.assertEqual(len(heart_checks), 2)
        cadence_checks = [
            s
            for s in ble['steps']
            if s.get('command') == 'RIDE SENSORS' and s.get('equals', {}).get('cadence') == 'Some(600)'
        ]
        self.assertEqual(len(cadence_checks), 2)
        late = ble['steps'].index({'op': 'command', 'command': 'BLE', 'baseline': 'heart_after_att_timeout'})
        self.assertGreater(sum(s['seconds'] for s in ble['steps'][:late] if s['op'] == 'delay'), 30)
        restore = harness.connectivity_restore_steps('ble', ble['connectivity_restore']['ble'])
        self.assertEqual([s['action'] for s in restore], ['select', 'echo'])
        self.assertEqual(restore[0]['mode'], 'authorized-heart')

    def att_timeout_hold(self, snapshots):
        """Run the BLE command checks from the first heart baseline through the hold."""
        steps = scenarios(Path('/private/profile.json'))['connectivity-ble']['steps']
        start = steps.index({'op': 'command', 'command': 'BLE', 'baseline': 'heart'})
        end = next(i for i, s in enumerate(steps) if i > start and s.get('action') == 'disconnect')
        checks = [s for s in steps[start:end] if s['op'] == 'command' and s['command'] == 'BLE']
        self.assertEqual(len(checks), len(snapshots))
        runner = object.__new__(harness.Runner)
        runner.observations = {}
        replies = [
            {'notifications': str(n), 'dropped': '0', 'connections': str(c), 'disconnections': str(d)}
            for n, c, d in snapshots
        ]
        with patch.object(runner, 'command', side_effect=replies):
            for index, step in enumerate(checks):
                runner.run_command(step, index)

    def test_ble_hold_past_att_timeout_passes_on_one_continuous_link(self):
        self.att_timeout_hold([(10, 1, 0), (16, 1, 0), (80, 1, 0), (86, 1, 0), (87, 1, 0)])

    def test_ble_hold_past_att_timeout_fails_on_a_reconnect_during_the_hold(self):
        with self.assertRaisesRegex(AssertionError, 'acquisition failed progress/loss assertions'):
            self.att_timeout_hold([(10, 1, 0), (16, 1, 0), (30, 2, 1), (36, 2, 1), (37, 2, 1)])


if __name__ == '__main__':
    unittest.main()
