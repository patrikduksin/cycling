import stat
import tempfile
import unittest
from pathlib import Path

from cycling_devtools.connectivity.pair import pair, save
from cycling_devtools.connectivity.vana_link import parse_pairing

URL = 'vanaride://pair?id=0a1b2c3d&k=AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8'


class FakeConnection:
    def __init__(self, *replies):
        self.replies = list(replies)
        self.commands = []

    def terminal_command(self, command):
        self.commands.append(command)
        status, data = self.replies.pop(0)
        return {'status': status, 'data': data}


class Clock:
    def __init__(self):
        self.now = 0.0

    def __call__(self):
        return self.now

    def sleep(self, seconds):
        self.now += seconds


def run(connection):
    clock = Clock()
    return pair(connection, sleep=clock.sleep, clock=clock)


class PairTest(unittest.TestCase):
    def test_pairing_waits_until_the_device_saved_the_new_id(self):
        connection = FakeConnection(
            ('BUSY', 'companion check'),
            ('ACCEPTED', f'id=0a1b2c3d url={URL}'),
            ('OK', 'ble=idle paired=pending'),
            ('OK', 'ble=advertising paired=0a1b2c3d'),
        )
        self.assertEqual(run(connection), parse_pairing(URL))
        self.assertEqual(connection.commands, ['PHONE PAIR', 'PHONE PAIR', 'PHONE', 'PHONE'])

    def test_paired_none_after_pair_means_the_save_failed(self):
        connection = FakeConnection(('ACCEPTED', f'id=0a1b2c3d url={URL}'), ('OK', 'paired=none'))
        with self.assertRaisesRegex(RuntimeError, 'did not save'):
            run(connection)

    def test_a_pairing_still_pending_after_five_seconds_times_out(self):
        connection = FakeConnection(('ACCEPTED', f'id=0a1b2c3d url={URL}'), *[('OK', 'paired=pending')] * 40)
        with self.assertRaises(TimeoutError):
            run(connection)

    def test_a_reply_whose_id_disagrees_with_its_url_is_refused(self):
        connection = FakeConnection(('ACCEPTED', f'id=ffffffff url={URL}'))
        with self.assertRaisesRegex(RuntimeError, 'ffffffff'):
            run(connection)

    def test_the_saved_url_is_private(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / 'pairing/vana.url'
            save(path, URL)
            save(path, URL)
            self.assertEqual(parse_pairing(path.read_text()), parse_pairing(URL))
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
            self.assertEqual(stat.S_IMODE(path.parent.stat().st_mode), 0o700)


if __name__ == '__main__':
    unittest.main()
