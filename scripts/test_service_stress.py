import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from service_stress import fields, validate, run

class ServiceStressTests(unittest.TestCase):
    def position(self, ms=1, **changes):
        result = dict(ms=ms, bytes='10', valid='1', checksum_errors='0', parse_errors='0',
                      dma_losses='0', line_overflows='0', uart_errors='0')
        result.update(changes)
        return result

    def test_reply_status_and_complete_counters_are_required(self):
        with self.assertRaises(AssertionError):
            fields(dict(status='UNAVAILABLE', data='', ms=1))
        before = self.position()
        after = self.position(ms=2, bytes='20', valid='2')
        del after['line_overflows']
        with self.assertRaises(KeyError):
            validate('POSITION', before, after)

    def test_no_fix_is_allowed_when_transport_and_parser_progress(self):
        delta = validate('POSITION', self.position(fix='no_fix'),
                         self.position(ms=2000, bytes='100', valid='5', fix='no_fix'))
        self.assertEqual(delta['valid'], 4)

    def test_loss_reset_and_absent_progress_fail(self):
        for after in (self.position(ms=2), self.position(ms=2, bytes='1'),
                      self.position(ms=2, bytes='20', valid='2', line_overflows='1')):
            with self.assertRaises(AssertionError):
                validate('POSITION', self.position(), after)
        delta = validate('POSITION', self.position(),
                         self.position(ms=2, bytes='20', valid='2', dma_losses='1'), True)
        self.assertEqual(delta['dma_losses'], 1)

    def test_input_overflow_is_reported_even_when_uart_is_clean(self):
        before = dict(ms=1, companion_valid='10', bad_crc='0', uart_errors='0', touch_errors='0', input_lost='0')
        after = dict(before, ms=2, companion_valid='20', input_lost='16')
        with self.assertRaises(AssertionError):
            validate('INPUT', before, after)

    def test_failed_run_retains_summary_and_raw_before_raising(self):
        settings = dict(brightness='50', dim_timeout='30', dim_brightness='20', timezone='0')
        system = dict(heap_free='1000', storage_ops='0')
        before = self.position()
        after = self.position(ms=2, bytes='20', valid='2', dma_losses='1')
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            raw = directory / 'raw.bin'
            raw.write_bytes(b'private capture remains')
            with patch('service_stress.private_directory', return_value=directory), \
                    patch('service_stress.UsbConnection'), \
                    patch('service_stress.read', side_effect=[
                        dict(cycling='false'), settings, system, before, after, settings, system]), \
                    patch('service_stress.time.monotonic', side_effect=[0, 0, 0, 3]), \
                    patch('service_stress.time.sleep'), \
                    self.assertRaisesRegex(AssertionError, 'faults advanced'):
                run('/dev/fake', directory, seconds=2)
            result = json.loads((directory / 'summary.json').read_text())
            self.assertFalse(result['ok'])
            self.assertEqual(result['before'], before)
            self.assertEqual(result['after'], after)
            self.assertEqual(result['delta']['dma_losses'], 1)
            self.assertEqual(result['error']['type'], 'AssertionError')
            self.assertEqual(raw.read_bytes(), b'private capture remains')

if __name__ == '__main__':
    unittest.main()
