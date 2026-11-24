import pytest

from cycling_devtools.terminal import time_sync


class Console:
    def __init__(self, set_status='OK', busy=0):
        self.sent = []
        self.set_status = set_status
        self.busy = busy

    def terminal_command(self, command):
        self.sent.append(command)
        if command.startswith('TIME SET'):
            if self.busy:
                self.busy -= 1
                return {'status': 'BUSY', 'data': 'starting companion=probing'}
            return {'status': self.set_status, 'data': f'unix_seconds={command.split()[-1]}'}
        return {'status': 'OK', 'data': 'Snapshot { status: Fresh } source=set'}


def test_the_set_goes_out_when_the_next_second_begins():
    slept = []
    assert time_sync.command(lambda: 1_793_521_799.25, slept.append) == 'TIME SET 1793521800'
    assert slept == [0.75]


def test_sync_sets_then_reads_the_clock_back():
    console = Console()
    reply = time_sync.sync(console, lambda: 1_793_521_800.0, lambda _: None)
    assert console.sent == ['TIME SET 1793521800', 'TIME']
    assert 'source=set' in reply['data']


def test_an_image_without_time_set_fails_loudly():
    console = Console(set_status='INVALID')
    with pytest.raises(RuntimeError, match='INVALID'):
        time_sync.sync(console, lambda: 1_793_521_800.0, lambda _: None)
    assert console.sent == ['TIME SET 1793521800']


def test_a_device_still_starting_is_asked_again():
    console = Console(busy=2)
    time_sync.sync(console, lambda: 1_793_521_800.0, lambda _: None)
    assert console.sent == ['TIME SET 1793521800'] * 3 + ['TIME']


def test_a_device_that_stays_busy_fails_loudly():
    console = Console(busy=time_sync.BUSY_TRIES)
    with pytest.raises(RuntimeError, match='BUSY'):
        time_sync.sync(console, lambda: 1_793_521_800.0, lambda _: None)
