import json

import pytest

from cycling_devtools import deep_tier, forge, nightly


class Issues(forge.Issues):
    def __init__(self, filed=()):
        self.marks, self.created = set(filed), []

    def filed(self, mark):
        return mark in self.marks

    def file(self, title, body):
        self.created.append(title)
        self.marks.add(body.split('\n', 1)[0])


class Device:
    def __init__(self, names, faithful=None, broken=()):
        self.names, self.faithful, self.broken = names, faithful or {}, set(broken)
        self.calls, self.remote = [], []

    def __call__(self, args, cwd):
        self.remote.append(args[1] == 'device')
        if args[1] == 'device':
            args = [args[0], *args[2:]]
        self.calls.append(args[1:])
        if args[1:3] == ['store', 'list']:
            if self.names is None:
                return 2, {'error': 'no device on USB'}
            return 0, [{'name': f'traces/{name}', 'id': i, 'chunks': 1} for i, name in enumerate(self.names)]
        if args[1:3] == ['trace', 'pull']:
            name = args[3]
            return 0, {'path': f'/p/.local/traces/{name}', 'faithful': self.faithful.get(name, {'ok': True})}
        if args[1] == 'corpus':
            pulled = [call[2] for call in self.calls if call[:2] == ['trace', 'pull']]
            verdict = lambda name: 'broken' if name in self.broken else 'passed'  # noqa: E731
            return 0, {'replayed': [{'trace': f'/p/.local/traces/{n}', 'verdict': verdict(n)} for n in pulled]}
        raise AssertionError(args)


@pytest.fixture
def state(tmp_path, monkeypatch):
    path = tmp_path / 'nightly' / 'state.json'
    monkeypatch.delenv('CYCLING_DEVICE_HOST', raising=False)
    monkeypatch.setattr(nightly, 'state_file', lambda root: path)
    return path


def run(device, issues):
    return nightly.nightly(run=device, issues=issues, deep=lambda issues: [])


def test_it_pulls_each_trace_newer_than_the_last_but_the_one_recording_now(state):
    state.parent.mkdir()
    state.write_text(json.dumps({'last': 'traces/000010-aa.trace'}))
    device = Device(['000009-ff.trace', '000010-aa.trace', '000011-bb.trace', '000012-cc.trace'])
    result = run(device, Issues())
    assert [p['trace'] for p in result['pulled']] == ['traces/000011-bb.trace']
    assert json.loads(state.read_text()) == {'last': 'traces/000011-bb.trace'}
    assert not any(device.remote), 'without CYCLING_DEVICE_HOST the C606 is on this machine'
    assert result == {**result, 'filed': [], 'deep': []}
    assert run(Device(['000011-bb.trace', '000012-cc.trace']), Issues())['pulled'] == []


def test_a_device_on_another_machine_is_listed_and_pulled_through_cycling_device(state, monkeypatch):
    monkeypatch.setenv('CYCLING_DEVICE_HOST', 'c606-host')
    seen = []
    device = Device(['000001-a.trace', '000002-now.trace'])

    def remote(args, cwd):
        seen.append(args[:3])
        return device(args, cwd)

    result = run(remote, Issues())
    assert [p['trace'] for p in result['pulled']] == ['traces/000001-a.trace']
    assert seen[:2] == [['cycling', 'device', 'store'], ['cycling', 'device', 'trace']]
    assert seen[2] == ['cycling', 'corpus', '--last'], 'the replay of the pulled rides stays here'


def test_a_divergence_or_a_broken_ride_files_one_issue_and_a_stop_files_none(state):
    names = ['000001-a.trace', '000002-b.trace', '000003-c.trace', '000004-now.trace']
    faithful = {'000001-a.trace': {'ok': False, 'ended': 'a recording bug at 00:12.000'}}
    device = Device(names, faithful=faithful, broken={'000002-b.trace'})
    issues = Issues()
    result = run(device, issues)
    assert result['filed'] == ['traces/000001-a.trace divergence', '/p/.local/traces/000002-b.trace broken']
    assert len(issues.created) == 2
    state.unlink()
    again = run(Device(names, faithful=faithful, broken={'000002-b.trace'}), issues)
    assert again['filed'] == [] and len(issues.created) == 2, 'the markers keep each issue to one'


def test_no_device_skips_the_pull_and_still_asks_for_tier_2(state):
    issues = Issues()
    asked = []

    def deep(given):
        asked.append(given)
        return [deep_tier.Run('abc', deep_tier.GateFailed('fuzz'), 'def')]

    result = nightly.nightly(run=Device(None), issues=issues, deep=deep)
    assert result['skipped'].startswith('the device did not list its traces')
    assert result['deep'] == [{'commit': 'abc', 'failed': 'fuzz', 'first_bad': 'def'}]
    assert asked == [issues] and not state.exists()


GREEN = {'commit': 'abc', 'failed': None, 'first_bad': None}
RED = {'commit': 'abc', 'failed': 'fuzz', 'first_bad': None}


@pytest.mark.parametrize(
    ('result', 'status'),
    [
        ({'pulled': [], 'filed': [], 'deep': [GREEN]}, 0),
        ({'pulled': [], 'filed': [], 'deep': []}, 0),
        ({'pulled': [], 'filed': ['t broken'], 'deep': [GREEN]}, 1),
        ({'pulled': [], 'filed': [], 'deep': [GREEN, RED]}, 1),
        ({'skipped': 'the device did not list its traces: no device on USB', 'deep': [RED]}, 1),
        ({'skipped': 'the device did not list its traces: no device on USB', 'deep': []}, 0),
    ],
)
def test_the_exit_status_fails_on_a_filed_issue_or_a_failed_tier_2_run(monkeypatch, result, status):
    monkeypatch.setattr(nightly, 'nightly', lambda: result)
    assert nightly.main(['--json']) == status


def test_install_writes_a_0300_timer_and_its_service_under_xdg_config_home(tmp_path):
    config = tmp_path / 'config'
    ran = []
    directory = nightly.install(
        environ={'XDG_CONFIG_HOME': str(config), 'HOME': '/nowhere', 'CYCLING_DEVICE_HOST': 'c606-host'},
        run=lambda a, **_: ran.append(a),
    )
    assert directory == config / 'systemd' / 'user'
    timer = (directory / 'cycling-nightly.timer').read_text()
    service = (directory / 'cycling-nightly.service').read_text()
    assert 'OnCalendar=*-*-* 03:00:00' in timer and 'WantedBy=timers.target' in timer
    assert '--command cycling trace nightly' in service and 'Type=oneshot' in service
    assert 'Environment=CYCLING_DEVICE_HOST=c606-host' in service, 'a boot without a desktop session still finds it'
    assert ran == [
        ['systemctl', '--user', 'daemon-reload'],
        ['systemctl', '--user', 'enable', '--now', 'cycling-nightly.timer'],
    ]
