import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from cycling_devtools.connectivity.ant import capture, send_command


class Terminal:
    def __init__(self, fail_send=False):
        self.commands = []
        self.fail_send = fail_send
        self.packets = 0

    def terminal_command(self, command):
        self.commands.append(command)
        status, data = 'OK', ''
        if command == 'ANT':
            data = 'scanning=false '
        elif command.startswith('ANT CHANNEL'):
            self.packets += 1
            data = ('Snapshot { selected: Some(Identity { device_type: 120, device_number: 1234, '
                    'transmission_type: 1 }), generation: 2, packets: ' + str(self.packets) +
                    ', dropped_packets: 0, transport_losses: 0, tx_failures: 0 }')
        elif command == 'ANT READ':
            status = 'EMPTY'
        elif command.startswith(('ANT SCAN ', 'ANT STOP', 'ANT SEND ')):
            if command.startswith('ANT SEND ') and self.fail_send:
                raise TimeoutError('uncertain')
            status, data = 'ACCEPTED', 'operation=3 stage=queued delivery=unknown'
        return dict(status=status, data=data)


class AntDiagnosticsTests(unittest.TestCase):
    def run_capture(self, terminal):
        with tempfile.TemporaryDirectory() as directory:
            ticks = iter(i * .5 for i in range(10000))
            with patch('cycling_devtools.connectivity.ant.time.monotonic', side_effect=lambda: next(ticks)), \
                 patch('cycling_devtools.connectivity.ant.time.sleep'):
                try:
                    report = capture(terminal, Path(directory), [120], 2, 2,
                                     send_command('120 1234 1 2 0102030405060708'))
                finally:
                    self.assertTrue((Path(directory) / 'report.json').exists())
                    for line in (Path(directory) / 'observations.jsonl').read_text().splitlines():
                        json.loads(line)
                return report

    def test_capture_uses_independent_reads_and_reports_each_window(self):
        terminal = Terminal()
        report = self.run_capture(terminal)
        self.assertEqual(sum(command.startswith('ANT SEND ') for command in terminal.commands), 1)
        self.assertIn('ANT OPERATION 3', terminal.commands)
        self.assertEqual(terminal.commands.count('ANT STOP'), 1)
        self.assertEqual(report['send_delivery'], 'unknown')
        self.assertTrue({'baseline', 'discovery', 'post_stop'} <= {row['phase'] for row in report['measurements']})

    def test_uncertain_send_is_never_replayed(self):
        terminal = Terminal(fail_send=True)
        with self.assertRaises(TimeoutError):
            self.run_capture(terminal)
        self.assertEqual(sum(command.startswith('ANT SEND ') for command in terminal.commands), 1)


if __name__ == '__main__':
    unittest.main()
