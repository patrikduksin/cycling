import unittest
from unittest.mock import patch
from base_test import apply, expect, preserved, wifi_failures, transport_recovery

class Connection:
    def __init__(self, status='OK'):
        self.request_id = 5
        self.status = status
        self.commands = []

    def terminal_command(self, command):
        self.commands.append(command)
        if command == 'SAVE':
            raise TimeoutError('completion unknown')
        return dict(status=self.status, data='stats=(2, 3, 4, 0)')

class BaseTests(unittest.TestCase):
    def test_ambiguous_save_is_not_replayed(self):
        connection = Connection()
        settings = dict(brightness='50', dim_timeout='30', dim_brightness='20', timezone='-180')
        with self.assertRaises(TimeoutError):
            apply(connection, settings)
        self.assertEqual(connection.commands.count('SAVE'), 1)

    def test_invalid_parser_reply_uses_reserved_id_zero(self):
        connection = Connection('INVALID')
        expect(connection, 'SAVE extra', 'INVALID')
        self.assertEqual(connection.request_id, -1) # Client increments before transmission.

    def test_restore_checks_every_persisted_preference(self):
        original = dict(brightness='50', dim_timeout='30', dim_brightness='20', timezone='-180')
        for key in original:
            with self.assertRaises(AssertionError):
                preserved(original, dict(original, **{key: 'wrong'}))

    def test_wifi_failure_counter_is_not_association_count(self):
        self.assertEqual(wifi_failures(Connection()), 4)

class StallConnection:
    def __init__(self, timeout=False):
        self.commands = []
        self.timeout = timeout

    def terminal_command(self, command, timeout=8):
        self.commands.append((command, timeout))
        if self.timeout:
            raise TimeoutError('stall completion unknown')
        return dict(status='ACCEPTED', ms=100, data='')

class TransportRecoveryTests(unittest.TestCase):
    def setUp(self):
        self.calls = 0

    def read(self, connection, command):
        self.calls += 1
        now = self.calls * 500
        stalled = bool(connection.commands)
        if command == 'POSITION':
            return dict(ms=now, transport='receiving', fix='no_fix', bytes=str(now),
                        valid=str(self.calls), checksum_errors='0', parse_errors='0',
                        dma_losses=str(int(stalled)), line_overflows='0', uart_errors='0')
        if command == 'INPUT':
            return dict(ms=now, companion_valid=str(self.calls), bad_crc='0',
                        uart_errors=str(int(stalled)), touch_errors='0', input_lost='0')
        if command == 'SETTINGS':
            return dict(brightness='50', dim_timeout='30', dim_brightness='20', timezone='-180')
        return dict(ms=now, storage_ops='0')

    def test_stalls_once_and_records_loss_without_requiring_an_indoor_fix(self):
        connection = StallConnection()
        report = {}
        with patch('base_test.read', side_effect=self.read), patch('base_test.time.sleep'):
            transport_recovery(connection, dict(cycling='false', harness='true'), report)
        self.assertEqual(connection.commands, [('TEST 20', 12)])
        self.assertTrue(report['recovered'])
        self.assertEqual(report['position_delta']['dma_losses'], 1)
        self.assertEqual(report['input_delta']['uart_errors'], 1)
        self.assertEqual(report['settled_position']['fix'], 'no_fix')

    def test_timeout_never_replays_stall_and_preserves_baseline(self):
        connection = StallConnection(timeout=True)
        report = {}
        with patch('base_test.read', side_effect=self.read), self.assertRaises(TimeoutError):
            transport_recovery(connection, dict(cycling='false', harness='true'), report)
        self.assertEqual(connection.commands, [('TEST 20', 12)])
        self.assertIn('before_position', report)
        self.assertFalse(report['recovered'])

    def test_sdk_build_is_rejected_before_any_command(self):
        connection = StallConnection()
        with self.assertRaises(ValueError):
            transport_recovery(connection, dict(cycling='true', harness='true'), {})
        self.assertEqual(connection.commands, [])

    def test_persistent_failure_is_not_treated_as_recovery(self):
        def failed(connection, command):
            result = self.read(connection, command)
            if command == 'POSITION':
                result['transport'] = 'failed'
            return result
        report = {}
        with patch('base_test.read', side_effect=failed), \
                patch('base_test.time.monotonic', side_effect=[0, 16]), self.assertRaises(TimeoutError):
            transport_recovery(StallConnection(), dict(cycling='false', harness='true'), report)
        self.assertFalse(report['recovered'])

if __name__ == '__main__':
    unittest.main()
