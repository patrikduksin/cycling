"""`cycling trace nightly`: tier 2's pull of the day's rides, with no one at the device, then tier 2 itself.

When the C606 answers on USB, here or on CYCLING_DEVICE_HOST through `cycling device`, it lists the STORE's
traces and pulls each one newer than the last it pulled, leaving out the newest, the session the device is
recording now. Each pull replays faithfully under the build that recorded it (`cycling trace pull`), and the
new rides replay under this checkout's build with broken() (`cycling corpus`). Then, device or not, it asks
for tier 2 as `cycling merge` does: `cycling verify --deep` on the newest main, coalesced with a run already
under way (`deep_tier`). Nothing presses a button.

A faithfulness divergence, a recording bug, or a broken invariant opens a GitHub issue with the replay's output
in it, once: the issue body carries a marker per trace and failure, and an open or closed issue with it is
never filed again. A stop under this build files nothing: a legitimate change to what VANA asks makes every
ride stop. It exits with 1 when it filed an issue or a tier 2 run it made failed.
The state, the last trace pulled, is in the primary checkout's .local/nightly/.

`--install` writes a systemd user timer for 03:00 local time, and its service, under $XDG_CONFIG_HOME
(~/.config by default), and enables it.
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

from cycling_devtools import deep_tier, forge, workspace

TRACES = 'traces/'
UNIT = 'cycling-nightly'


def run_json(args, cwd):
    done = subprocess.run(args, cwd=cwd, stdin=subprocess.DEVNULL, capture_output=True, text=True)
    try:
        return done.returncode, json.loads(done.stdout)
    except json.JSONDecodeError:
        return done.returncode, {'error': done.stderr.strip() or f'{" ".join(args)} printed no JSON'}


def on_device(args):
    return ['cycling', 'device', *args[1:]] if os.environ.get('CYCLING_DEVICE_HOST') else args


def state_file(root):
    return workspace.main_checkout(root) / '.local' / 'nightly' / 'state.json'


def sequence(name):
    return name.rsplit('/', 1)[-1].split('-')[0]


def marker(trace, kind):
    return f'<!-- cycling-nightly {trace} {kind} -->'


def report(issues, trace, kind, title, detail):
    body = f'The nightly pull of `{trace}` found this.\n\n```json\n{json.dumps(detail, indent=2)}\n```\n'
    return issues.file_once(marker(trace, kind), title, body)


def nightly(root=workspace.ROOT, *, run=run_json, issues=None, deep=deep_tier.request):
    issues = issues or forge.Issues()
    status, listed = run(on_device(['cycling', 'store', 'list', TRACES]), root)
    if status == 0 and isinstance(listed, list):
        result = pull(root, run, issues, listed)
    else:
        result = {'skipped': f'the device did not list its traces: {listed.get("error", listed)}'}
    result['deep'] = [done.json() for done in deep(issues)]
    return result


def pull(root, run, issues, listed):
    state_path = state_file(root)
    state = json.loads(state_path.read_text()) if state_path.exists() else {}
    names = sorted((entry['name'] for entry in listed), key=sequence)[:-1]
    last = state.get('last')
    fresh = [name for name in names if last is None or sequence(name) > sequence(last)]
    pulled, filed = [], []
    for name in fresh:
        _, result = run(on_device(['cycling', 'trace', 'pull', name.rsplit('/', 1)[-1], '--json']), root)
        if 'error' in result:
            pulled.append({'trace': name, 'error': result['error']})
            break
        faithful = result.get('faithful', {})
        if faithful.get('ok') is False and report(
            issues, name, 'divergence', f'Nightly: {name} does not replay faithfully', faithful
        ):
            filed.append(f'{name} divergence')
        pulled.append({'trace': name, 'path': result['path'], 'faithful': faithful})
        state['last'] = name
        state_path.parent.mkdir(parents=True, exist_ok=True)
        state_path.write_text(json.dumps(state))
    if pulled:
        _, replayed = run(['cycling', 'corpus', '--last', str(len(pulled)), '--json'], root)
        for result in replayed.get('replayed', []):
            if result['verdict'] == 'broken' and report(
                issues, result['trace'], 'broken', f'Nightly: VANA breaks on {Path(result["trace"]).name}', result
            ):
                filed.append(f'{result["trace"]} broken')
    return {'pulled': pulled, 'filed': filed}


def units(root, nix, host=None):
    primary = workspace.main_checkout(root)
    environment = f'Environment=CYCLING_DEVICE_HOST={host}\n' if host else ''
    service = f"""[Unit]
Description=cycling nightly: pull the day's rides, check them, then cycling verify --deep

[Service]
Type=oneshot
WorkingDirectory={primary}
ExecStart={nix} develop {primary} --command cycling trace nightly
{environment}Nice=19
"""
    timer = """[Unit]
Description=cycling nightly at 03:00

[Timer]
OnCalendar=*-*-* 03:00:00
Persistent=true

[Install]
WantedBy=timers.target
"""
    return {f'{UNIT}.service': service, f'{UNIT}.timer': timer}


def install(root=workspace.ROOT, environ=os.environ, run=subprocess.run):
    config = Path(environ.get('XDG_CONFIG_HOME') or Path(environ['HOME']) / '.config')
    directory = config / 'systemd' / 'user'
    directory.mkdir(parents=True, exist_ok=True)
    for name, text in units(root, shutil.which('nix') or 'nix', environ.get('CYCLING_DEVICE_HOST')).items():
        (directory / name).write_text(text)
    run(['systemctl', '--user', 'daemon-reload'], check=True)
    run(['systemctl', '--user', 'enable', '--now', f'{UNIT}.timer'], check=True)
    return directory


def main(argv=None):
    parser = argparse.ArgumentParser(prog='cycling trace nightly', description=__doc__)
    parser.add_argument('--install', action='store_true', help='write and enable the 03:00 systemd user timer')
    parser.add_argument('--json', action='store_true')
    args = parser.parse_args(argv)
    if args.install:
        print(f'installed {UNIT}.timer in {install()}')
        return 0
    result = nightly()
    print(json.dumps(result) if args.json else json.dumps(result, indent=2))
    return 1 if result.get('filed') or any(done['failed'] for done in result['deep']) else 0


if __name__ == '__main__':
    sys.exit(main())
