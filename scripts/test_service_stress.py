import unittest
from service_stress import fields, validate

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

if __name__ == '__main__':
    unittest.main()
