import pytest

from cycling_devtools.device import format as device_format
from cycling_devtools.harness import base

CURRENT = 'state=idle table=current rides=current internal=current settings=blank'


class ScriptedDevice:
    def __init__(self, *replies):
        self.replies, self.sent = list(replies), []

    def __enter__(self):
        return self

    def __exit__(self, *_):
        return False

    def terminal_command(self, command):
        self.sent.append(command)
        reply = self.replies.pop(0)
        if isinstance(reply, Exception):
            raise reply
        return {'status': reply[0], 'data': reply[1]}


def wipe(monkeypatch, tmp_path, before, after):
    monkeypatch.setattr(device_format, 'UsbConnection', lambda *_: before)
    monkeypatch.setattr(base, 'reopen', lambda *_: after)
    monkeypatch.setattr(base.time, 'sleep', lambda _: None)
    return device_format.wipe('port', tmp_path)


def test_health_is_read_as_fields():
    device = ScriptedDevice(('OK', 'state=offered settings=foreign rides=current'))
    assert device_format.health(device) == {'state': 'offered', 'settings': 'foreign', 'rides': 'current'}


@pytest.mark.parametrize('gone', [OSError('gone'), RuntimeError('USB terminal closed'), TimeoutError()])
def test_a_wipe_waits_out_the_restart_and_the_stores_still_unread(monkeypatch, tmp_path, gone):
    before = ScriptedDevice(('ACCEPTED', ''), ('OK', 'state=running'), gone)
    after = ScriptedDevice(('OK', 'state=idle table=unread internal=unread'), ('OK', CURRENT))
    assert wipe(monkeypatch, tmp_path, before, after)['settings'] == 'blank'
    assert before.sent == ['FORMAT CONFIRM', 'FORMAT', 'FORMAT']
    assert after.sent == ['FORMAT', 'FORMAT']


def test_a_wipe_that_never_settles_times_out(monkeypatch, tmp_path):
    clock = iter(range(0, 10_000, 30))
    monkeypatch.setattr(base.time, 'monotonic', lambda: next(clock))
    before = ScriptedDevice(('ACCEPTED', ''), *[('OK', 'state=running')] * 10)
    with pytest.raises(TimeoutError, match='format'):
        wipe(monkeypatch, tmp_path, before, ScriptedDevice())


@pytest.mark.parametrize(
    ('before', 'after', 'message'),
    [
        ((('STATE', ''),), (), 'FORMAT CONFIRM: STATE'),
        ((('ACCEPTED', ''), ('OK', 'state=failed')), (), 'the wipe failed'),
        ((('ACCEPTED', ''), OSError('gone')), (('OK', 'state=offered settings=foreign'),), 'settings'),
    ],
)
def test_a_wipe_that_does_not_finish_is_an_error(monkeypatch, tmp_path, before, after, message):
    with pytest.raises(RuntimeError, match=message):
        wipe(monkeypatch, tmp_path, ScriptedDevice(*before), ScriptedDevice(*after))
