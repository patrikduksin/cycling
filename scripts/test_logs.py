import json
import unittest
from unittest.mock import patch
import termios
from logs import Decoder, MAX_LINE, open_no_reset


def log(boot=1, seq=0, **fields):
    return (json.dumps(dict(type='log', boot=boot, seq=seq, **fields)) + '\n').encode()


class LogsTests(unittest.TestCase):
    def setUp(self):
        self.records = []
        self.d = Decoder(self.records.append)

    def events(self):
        return [r.get('event') for r in self.records if r['type'] == 'host']

    def test_partial_boot_gap_and_wrap(self):
        data = log(seq=0xffffffff)
        self.d.feed(data[:10]); self.assertFalse(self.records)
        self.d.feed(data[10:] + log(seq=0) + log(seq=3) + log(boot=2))
        self.assertEqual(self.events(), ['boot_boundary', 'sequence_gap', 'boot_boundary'])
        self.assertEqual(self.records[3]['missing'], 2)

    def test_reconnect_does_not_imply_reboot_or_join_fragments(self):
        self.d.feed(log() + b'{"type":')
        self.d.boundary('disconnected')
        self.d.boundary('connected')
        self.d.feed(log(seq=1))
        self.assertEqual(self.events().count('boot_boundary'), 1)
        self.assertIn('truncated', self.events())

    def test_long_invalid_utf8_and_reply(self):
        self.d.feed(b'x' * (MAX_LINE + 20))
        self.assertLessEqual(len(self.d.pending), MAX_LINE)
        self.d.feed(b'\n\xff\n{"type":"reply","id":7}\n' + log())
        self.assertEqual(self.d.counts['overlong'], 1)
        self.assertEqual(self.d.counts['unstructured'], 1)
        self.assertEqual(self.d.counts['replies'], 1)
        self.assertEqual(self.d.counts['logs'], 1)

    def test_receipt_time_preserves_device_time(self):
        with patch("logs.time.time_ns", return_value=123456):
            self.d.feed(log(ms=789))
        self.assertEqual(self.records[-1]["host_ns"], 123456)
        self.assertEqual(self.records[-1]["ms"], 789)

    def test_bad_schema_is_rejected(self):
        self.d.feed(log(seq=True) + log(boot='private') + b'[]\n')
        self.assertEqual(self.d.counts['malformed_log'], 2)
        self.assertEqual(self.d.counts['logs'], 0)

    def test_open_no_flush_or_modem_calls(self):
        attrs = [0, 0, termios.HUPCL, 0, 0, 0, [b'\0'] * 32]
        with patch('logs.os.open', return_value=5), patch('logs.termios.tcgetattr', return_value=attrs), \
                patch('logs.termios.tcsetattr') as setter, patch('logs.tty.cfmakeraw') as raw:
            self.assertEqual(open_no_reset('/dev/fake'), 5)
            raw.assert_called_once_with(attrs)
            setter.assert_called_once_with(5, termios.TCSANOW, attrs)
            self.assertFalse(attrs[2] & termios.HUPCL)
            self.assertTrue(attrs[2] & termios.CLOCAL)


if __name__ == '__main__':
    unittest.main()
