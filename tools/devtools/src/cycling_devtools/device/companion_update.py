"""Observe the self-contained same-version N22 update experiment after cycling flash.

Build with CYCLING_COMPANION_STOCK pointing at the private, verified 1.902
container, then cycling [device] flash --companion-update. The build script
pins its SHA-256. This observer never streams vendor bytes or replays a start.
"""

import re
import time

from cycling_devtools.terminal.usb import UsbConnection
from cycling_devtools.workspace import ROOT


def require_reply(connection, command):
    reply = connection.terminal_command(command)
    if reply['status'] not in {'OK', 'ACCEPTED'}:
        raise RuntimeError(f'{command}: {reply}')
    return reply['data']


def run(port):
    output = ROOT / '.local/companion-update' / str(time.time_ns())
    output.mkdir(parents=True, mode=0o700)
    print(f'Companion experiment evidence: {output}', flush=True)
    with UsbConnection(port, output / 'usb.log') as connection:
        # INFO can answer before the companion's startup check finishes. BUSY
        # here guarantees SETTINGS ran nothing; only this read is retried.
        deadline = time.monotonic() + 20
        while True:
            ready = connection.terminal_command('SETTINGS')
            if ready['status'] == 'OK':
                break
            if ready['status'] != 'BUSY' or not ready['data'].startswith('starting ') or time.monotonic() >= deadline:
                raise RuntimeError(f'Device not ready; no DFU command sent: {ready}')
            time.sleep(0.1)
        status = require_reply(connection, 'TEST 43')
        if not status.startswith('phase=Ready '):
            raise RuntimeError(f'Update is not ready; no start sent: {status}')
        # Prove the app-mode query still works immediately before DFU. The
        # probe retains only eight non-report frames, so inspect its timestamp
        # against the pre-query reply, rather than accepting an old answer.
        before = connection.terminal_command('TEST 41')
        if before['status'] != 'OK':
            raise RuntimeError(f'Probe unavailable: {before}')
        require_reply(connection, 'TEST 40')
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            probe = require_reply(connection, 'TEST 41')
            seen = re.findall(r'(\d+)ms/\d+:([0-9a-f]+)', probe)
            if any(
                int(at) >= before['ms']
                and len(bytes.fromhex(raw)) >= 14
                and bytes.fromhex(raw)[5:8] == b'\x01\x24\x01'
                and bytes.fromhex(raw)[11] == 1
                for at, raw in seen
            ):
                break
            time.sleep(0.1)
        else:
            raise RuntimeError('No fresh app-mode answer; companion was not put into DFU')
        print('Fresh app-mode reply verified; starting embedded stock 1.902 transfer.', flush=True)
        require_reply(connection, 'TEST 42')  # Exactly one submission.
        deadline = time.monotonic() + 240
        previous = None
        while time.monotonic() < deadline:
            status = require_reply(connection, 'TEST 43')
            if status != previous:
                print(status, flush=True)
                previous = status
            if status.startswith('phase=Complete '):
                (output / 'result.txt').write_text(status + '\n')
                print(
                    'Companion accepted the complete image and answered app mode. Restore the ordinary base build.',
                    flush=True,
                )
                return
            if status.startswith('phase=Failed '):
                (output / 'result.txt').write_text(status + '\n' + require_reply(connection, 'TEST 41') + '\n')
                raise RuntimeError(
                    'Companion update failed; retain the embedded recovery image and inspect the private evidence before further writes'
                )
            time.sleep(0.25)
        raise RuntimeError(
            'Observer deadline; update outcome is uncertain. The device retains the image and continues independently; inspect TEST 43 before any reset.'
        )


def run_trial(port):
    """Submit the trial once; reconnect only for observations of the ESP guard."""
    from cycling_devtools.device.port import DiscoveryError, resolve_port

    output = ROOT / '.local/companion-trial' / str(time.time_ns())
    output.mkdir(parents=True, mode=0o700)
    print(f'Companion trial evidence: {output}', flush=True)
    attempted = False
    custom_seen = False
    restore_seen = False
    previous = None
    deadline = time.monotonic() + 240
    attach = 0
    while time.monotonic() < deadline:
        try:
            # Re-discover the verified device after disconnect. Never guess a
            # replacement port, and never replay TEST 44 after an uncertain send.
            current = port if attach == 0 else str(resolve_port())
            attach += 1
            with UsbConnection(current, output / f'usb-{attach}.log') as connection:
                while time.monotonic() < deadline:
                    info = connection.terminal_command('TEST 43')
                    if info['status'] == 'BUSY' and info['data'].startswith('starting '):
                        time.sleep(0.25)
                        continue
                    if info['status'] in ('INVALID', 'OVERLONG'):
                        # USB re-enumeration can leave an incomplete console line.
                        # Only repeat this read; a trial start is never replayed.
                        print(f'Retrying read-only trial status after console rejection: {info}', flush=True)
                        time.sleep(0.25)
                        continue
                    if info['status'] != 'OK':
                        raise ValueError(f'Trial diagnostic unavailable: {info}')
                    status = info['data']
                    if ' trial=' not in status:
                        raise ValueError('Trial guard is not embedded; no custom update sent')
                    if status != previous:
                        print(status, flush=True)
                        previous = status
                    custom_seen |= 'custom_seen=true' in status
                    restore_seen |= 'trial=Restoring ' in status
                    if 'trial=Failed ' in status:
                        raise ValueError(
                            'Automatic stock restore failed; keep this recovery-enabled main image installed'
                        )
                    if attempted and ('trial=Restored ' in status or 'trial=Ready ' in status):
                        result = f'custom_observed={custom_seen} restore_observed={restore_seen} {status}\n'
                        (output / 'result.txt').write_text(result)
                        if not custom_seen:
                            raise ValueError('Stock is responding, but custom execution was not observed')
                        if not restore_seen:
                            raise ValueError('Stock is responding, but the timed restore was not observed')
                        print(
                            'Custom marker observed; timed stock reflash observed; stock identity returned.', flush=True
                        )
                        return
                    if not attempted and 'trial=Ready ' in status and status.startswith('phase=Ready '):
                        ready = connection.terminal_command('SETTINGS')
                        if ready['status'] == 'BUSY':
                            time.sleep(0.25)
                            continue
                        if ready['status'] != 'OK':
                            raise ValueError(f'Device not ready: {ready}')
                        attempted = True  # Before sending: even a timeout is never replayed.
                        require_reply(connection, 'TEST 44')
                        print('Fixed custom marker trial submitted once; ESP owns timed stock restoration.', flush=True)
                    time.sleep(0.25)
        except (OSError, RuntimeError, DiscoveryError) as error:
            # Every command after a potentially submitted start is read-only.
            if not attempted:
                raise
            print(f'Observer reconnecting for read-only status: {error}', flush=True)
            time.sleep(1)
    raise RuntimeError('Trial observer deadline; retain the recovery-enabled image and inspect the private evidence')
