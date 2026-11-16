import pytest

from cycling_devtools.device import format as device_format

CURRENT = 'state=idle settings=blank internal=current table=current rides=current'


class ScriptedConnection:
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
    monkeypatch.setattr(device_format, 'reopen', lambda *_: after)
    return device_format.wipe('port', tmp_path, sleep=lambda _: None)


def test_health_is_read_as_fields():
    device = ScriptedConnection(('OK', 'state=offered settings=foreign rides=current'))
    assert device_format.health(device) == {'state': 'offered', 'settings': 'foreign', 'rides': 'current'}


def test_a_wipe_waits_out_the_restart_and_reports_the_fresh_stores(monkeypatch, tmp_path):
    before = ScriptedConnection(('ACCEPTED', ''), ('OK', 'state=running'), OSError('gone'))
    after = ScriptedConnection(('OK', CURRENT))
    assert wipe(monkeypatch, tmp_path, before, after)['settings'] == 'blank'
    assert before.sent == ['FORMAT CONFIRM', 'FORMAT', 'FORMAT']


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
        wipe(monkeypatch, tmp_path, ScriptedConnection(*before), ScriptedConnection(*after))
