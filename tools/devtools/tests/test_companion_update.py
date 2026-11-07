"""Update observer must never replay DFU entry after uncertain completion."""

import pytest

from cycling_devtools.device import companion_update as update


class Connection:
    def __init__(self, replies):
        self.replies = iter(replies)
        self.commands = []

    def terminal_command(self, command):
        self.commands.append(command)
        reply = next(self.replies)
        if isinstance(reply, Exception):
            raise reply
        if isinstance(reply, dict):
            return reply
        return {'status': 'OK', 'data': reply, 'ms': 100}

    def __enter__(self):
        return self

    def __exit__(self, *_):
        pass


def setup(monkeypatch, tmp_path, replies):
    connection = Connection(['settings ready', *replies])
    monkeypatch.setattr(update, 'ROOT', tmp_path)
    monkeypatch.setattr(update, 'UsbConnection', lambda *_: connection)
    monkeypatch.setattr(update.time, 'sleep', lambda _: None)
    return connection


FRESH_APP = 'total=1 110ms/16:a50c6ff105012401ffff0001ffff0000'


def test_no_start_if_an_update_already_ran(monkeypatch, tmp_path):
    connection = setup(monkeypatch, tmp_path, ['phase=Failed retries=5'])
    with pytest.raises(RuntimeError, match='not ready'):
        update.run('unused')
    assert connection.commands == ['SETTINGS', 'TEST 43']


def test_start_is_sent_once_on_success(monkeypatch, tmp_path):
    connection = setup(
        monkeypatch,
        tmp_path,
        [
            'phase=Ready retries=0',
            'total=0',
            '',
            FRESH_APP,
            '',
            'phase=Data(0) retries=0',
            'phase=Complete retries=0',
        ],
    )
    update.run('unused')
    assert connection.commands.count('TEST 42') == 1
    assert next((tmp_path / '.local/companion-update').glob('*/result.txt')).read_text().startswith('phase=Complete ')


def test_uncertain_start_is_never_replayed(monkeypatch, tmp_path):
    connection = setup(
        monkeypatch,
        tmp_path,
        [
            'phase=Ready retries=0',
            'total=0',
            '',
            FRESH_APP,
            TimeoutError('uncertain'),
        ],
    )
    with pytest.raises(TimeoutError, match='uncertain'):
        update.run('unused')
    assert connection.commands.count('TEST 42') == 1


def test_failure_preserves_probe_and_does_not_reset(monkeypatch, tmp_path):
    connection = setup(
        monkeypatch,
        tmp_path,
        [
            'phase=Ready retries=0',
            'total=0',
            '',
            FRESH_APP,
            '',
            'phase=Failed retries=5',
            'private probe',
        ],
    )
    with pytest.raises(RuntimeError, match='retain the embedded recovery image'):
        update.run('unused')
    assert connection.commands.count('TEST 42') == 1
    assert connection.commands[-1] == 'TEST 41'
    assert 'private probe' in next((tmp_path / '.local/companion-update').glob('*/result.txt')).read_text()


@pytest.mark.parametrize(
    'probe',
    [
        FRESH_APP.replace('110ms', '90ms'),
        FRESH_APP.replace('0001ffff', '0000ffff'),
        'total=1 110ms/8:a50c6ff105012401',
    ],
)
def test_stale_bootloader_or_short_answer_cannot_start_dfu(monkeypatch, tmp_path, probe):
    connection = setup(monkeypatch, tmp_path, ['phase=Ready retries=0', 'total=0', '', probe])
    ticks = iter([0, 0, 0, 6])
    monkeypatch.setattr(update.time, 'monotonic', lambda: next(ticks))
    with pytest.raises(RuntimeError, match='No fresh app-mode answer'):
        update.run('unused')
    assert 'TEST 42' not in connection.commands


TRIAL_READY = 'phase=Ready trial=Ready custom_seen=false custom_at=-1 stock_seen=true target=stock'
TRIAL_CUSTOM = 'phase=Complete trial=Soak custom_seen=true custom_at=4210 stock_seen=false target=trial'
# The observer records the probe, then acknowledges: TEST 41, TEST 45.
RECORD = ['total=1 4210ms/18:custom identity', '']
TRIAL_RESTORE = 'phase=Data(100) trial=Restoring custom_seen=true custom_at=4210 stock_seen=false target=stock'
TRIAL_STOCK = 'phase=Ready trial=Ready custom_seen=false custom_at=-1 stock_seen=true target=stock'


def test_trial_records_custom_identity_before_acknowledging_restore(monkeypatch, tmp_path):
    connection = Connection([TRIAL_READY, 'settings ready', '', TRIAL_CUSTOM, *RECORD, TRIAL_RESTORE, TRIAL_STOCK])
    monkeypatch.setattr(update, 'ROOT', tmp_path)
    monkeypatch.setattr(update, 'UsbConnection', lambda *_: connection)
    monkeypatch.setattr(update.time, 'sleep', lambda _: None)
    update.run_trial('unused')
    assert connection.commands.count('TEST 44') == 1
    assert 'TEST 42' not in connection.commands
    assert connection.commands[-4:-1] == ['TEST 41', 'TEST 45', 'TEST 43']
    custom = next((tmp_path / '.local/companion-trial').glob('*/custom.txt')).read_text()
    assert 'custom_at=4210' in custom and 'custom identity' in custom
    assert (
        next((tmp_path / '.local/companion-trial').glob('*/result.txt'))
        .read_text()
        .startswith("outcome='candidate answered; stock restored' ")
    )


def test_trial_never_replays_uncertain_start_across_reconnect(monkeypatch, tmp_path):
    from cycling_devtools.device import port

    first = Connection([TRIAL_READY, 'settings ready', TimeoutError('uncertain start')])
    second = Connection([TRIAL_CUSTOM, *RECORD, TRIAL_RESTORE, TRIAL_STOCK])
    connections = iter([first, second])
    monkeypatch.setattr(update, 'ROOT', tmp_path)
    monkeypatch.setattr(update, 'UsbConnection', lambda *_: next(connections))
    monkeypatch.setattr(port, 'resolve_port', lambda: 'verified-device')
    monkeypatch.setattr(update.time, 'sleep', lambda _: None)
    update.run_trial('unused')
    assert first.commands.count('TEST 44') == 1
    assert second.commands == ['TEST 43', 'TEST 41', 'TEST 45', 'TEST 43', 'TEST 43']


def test_trial_reports_a_candidate_that_never_answered(monkeypatch, tmp_path):
    connection = Connection(
        [TRIAL_READY, 'settings ready', '', TRIAL_RESTORE.replace('custom_seen=true', 'custom_seen=false'), TRIAL_STOCK]
    )
    monkeypatch.setattr(update, 'ROOT', tmp_path)
    monkeypatch.setattr(update, 'UsbConnection', lambda *_: connection)
    monkeypatch.setattr(update.time, 'sleep', lambda _: None)
    update.run_trial('unused')
    assert connection.commands.count('TEST 44') == 1
    result = next((tmp_path / '.local/companion-trial').glob('*/result.txt')).read_text()
    assert result.startswith("outcome='candidate never answered; guard restored stock'")


def test_trial_reports_a_candidate_running_with_the_stock_identity(monkeypatch, tmp_path):
    connection = Connection([TRIAL_READY, 'settings ready', '', TRIAL_STOCK])
    monkeypatch.setattr(update, 'ROOT', tmp_path)
    monkeypatch.setattr(update, 'UsbConnection', lambda *_: connection)
    monkeypatch.setattr(update.time, 'sleep', lambda _: None)
    update.run_trial('unused')
    result = next((tmp_path / '.local/companion-trial').glob('*/result.txt')).read_text()
    assert 'runs as stock' in result


def test_trial_retries_rejected_status_without_replaying_start(monkeypatch, tmp_path):
    connection = Connection(
        [
            TRIAL_READY,
            'settings ready',
            '',
            {'status': 'INVALID', 'data': 'malformed command', 'ms': 1727},
            TRIAL_CUSTOM,
            *RECORD,
            TRIAL_RESTORE,
            TRIAL_STOCK,
        ]
    )
    monkeypatch.setattr(update, 'ROOT', tmp_path)
    monkeypatch.setattr(update, 'UsbConnection', lambda *_: connection)
    monkeypatch.setattr(update.time, 'sleep', lambda _: None)
    update.run_trial('unused')
    assert connection.commands.count('TEST 44') == 1
    assert connection.commands[3:] == ['TEST 43', 'TEST 43', 'TEST 41', 'TEST 45', 'TEST 43', 'TEST 43']
