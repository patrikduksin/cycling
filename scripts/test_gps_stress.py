import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import gps_stress


class FakeDevice:
    def __init__(self, _port, directory):
        self.directory = directory
        self.calls = 0

    def __enter__(self):
        return self

    def __exit__(self, *_args):
        return False

    def command(self, command):
        assert command == 'STATE'
        self.calls += 1
        value = self.calls
        return {
            'gps_bytes': value * 100,
            'gps_valid': value * 2,
            'gps_checksum_errors': 0,
            'gps_parse_errors': 0,
            'gps_overflows': 0,
            'gps_line_overflows': 0,
            'gps_uart_errors': 0,
            'recorded_rides': 4,
            'recording_slot': 22,
            'brightness': 50,
            'dim_timeout': 30,
            'dim_brightness': 10,
            'timezone': 60,
            'harness': True,
            'recording': False,
        }


class GpsStressTests(unittest.TestCase):
    def test_reports_progress_and_preserved_state(self):
        with tempfile.TemporaryDirectory() as temporary:
            ticks = iter((0, 0, 0.5, 1.1, 1.2))
            with patch.object(gps_stress, 'Device', FakeDevice), patch.object(
                    gps_stress.time, 'monotonic', side_effect=lambda: next(ticks)):
                result = gps_stress.run('unused', Path(temporary), 1, True)
            self.assertGreater(result['delta']['gps_bytes'], 0)
            self.assertEqual(result['delta']['gps_uart_errors'], 0)


if __name__ == '__main__':
    unittest.main()
