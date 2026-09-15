"""Bounded ANT capture through the production terminal API. Never retries sends.

Run with python -m cycling_devtools.connectivity.ant --help. All output is private.
The operator connects owned peers first; this tool never replaces their selection.
"""
import argparse
import json
from pathlib import Path
import re
import subprocess
import time

from cycling_devtools.terminal.usb import ROOT, UsbConnection


def send_command(value):
    fields = value.split()
    if len(fields) != 5:
        raise ValueError('send requires type number transmission generation hex16')
    kind, number, transmission, generation = map(int, fields[:4])
    if not (0 < kind <= 255 and 0 < number <= 65535 and 0 <= transmission <= 255
            and 0 <= generation <= 0xffffffff):
        raise ValueError('send target is out of range')
    if not re.fullmatch('[0-9a-fA-F]{16}', fields[4]):
        raise ValueError('send requires exactly eight hexadecimal bytes')
    return f'ANT SEND {kind} {number} {transmission} {generation} {fields[4]}'


def channel_fields(text):
    """Read the existing Snapshot debug wire representation without changing it."""
    result = {}
    for name in ('device_type', 'device_number', 'transmission_type', 'generation',
                 'packets', 'dropped_packets', 'transport_losses', 'tx_failures'):
        match = re.search(r'\b' + name + r': (\d+)', text)
        if match:
            result[name] = int(match[1])
    return result


def summarize(samples):
    peers = {}
    for sample in samples:
        if not sample['command'].startswith('ANT CHANNEL ') or sample['reply']['status'] != 'OK':
            continue
        fields = channel_fields(sample['reply']['data'])
        if not all(key in fields for key in ('device_type', 'device_number', 'transmission_type', 'generation', 'packets')):
            continue
        key = tuple(fields[name] for name in ('device_type', 'device_number', 'transmission_type', 'generation'))
        key = (sample['phase'], *key)
        peers.setdefault(key, []).append((sample['elapsed_s'], fields))
    results = []
    for key, values in peers.items():
        first_time, first = values[0]
        last_time, last = values[-1]
        duration = last_time - first_time
        delta = last['packets'] - first['packets']
        results.append(dict(phase=key[0], device_type=key[1], device_number=key[2],
            transmission_type=key[3], generation=key[4], duration_s=duration,
            packets_delta=delta, packets_per_s=delta / duration if duration > 0 and delta >= 0 else None,
            loss_delta={name: last.get(name, 0) - first.get(name, 0)
                        for name in ('dropped_packets', 'transport_losses', 'tx_failures')}))
    return results


def capture(connection, output, types, seconds, scan_seconds, send=None):
    """Bounded baseline/discovery/post-stop windows; one optional explicit send."""
    started = time.monotonic()
    samples = []
    report = {'measurements': [], 'send_delivery': 'unknown',
              'identity': 'host-selected correlation, not identity present on each received page',
              'limits': 'Rates use device counters; USB polling samples diagnostics and can miss pages. '
                        'A bridge reply does not establish radio delivery or sensor execution.'}
    scan_attempted = False
    stop_attempted = False
    phase = 'initial'
    with (output / 'observations.jsonl').open('x') as stream:
        def command(text, allowed=('OK',)):
            reply = connection.terminal_command(text)
            sample = dict(elapsed_s=time.monotonic() - started, phase=phase, command=text, reply=reply)
            samples.append(sample)
            stream.write(json.dumps(sample) + '\n')
            stream.flush()
            if reply['status'] not in allowed:
                raise RuntimeError(f'{text.split()[0:2]} returned {reply["status"]}')
            return reply

        def window(duration):
            deadline = time.monotonic() + duration
            while True:
                command('ANT CAPABILITIES')
                for kind in types:
                    command(f'ANT CHANNEL {kind}', ('OK', 'UNAVAILABLE'))
                # Bound draining so high-rate peers cannot postpone other observations.
                for _ in range(10):
                    if command('ANT READ', ('OK', 'EMPTY'))['status'] == 'EMPTY':
                        break
                if time.monotonic() >= deadline:
                    break
                time.sleep(min(.25, max(0, deadline - time.monotonic())))

        try:
            for text in ('INFO', 'COMPANION', 'ANT CAPABILITIES', 'ANT', 'BATTERY', 'INPUT'):
                initial = command(text)
                if text == 'ANT' and not initial['data'].startswith('scanning=false '):
                    raise RuntimeError('capture requires an initially idle discovery scan')
            phase = 'baseline'
            window(seconds)
            if scan_seconds:
                phase = 'discovery'
                scan_attempted = True
                command(f'ANT SCAN {scan_seconds}', ('ACCEPTED',))
                window(scan_seconds)
                command('ANT DEVICES')
                phase = 'post_stop'
                # The firmware duration is bounded. Explicit STOP provides an observation
                # boundary even when it has already expired; STATE means it was idle.
                stop_attempted = True
                command('ANT STOP', ('ACCEPTED', 'STATE'))
                window(seconds)
                final_scan = command('ANT')
                if not final_scan['data'].startswith('scanning=false '):
                    raise RuntimeError('discovery stop remains unconfirmed')
            if send:
                phase = 'send'
                result = command(send, ('ACCEPTED',))
                match = re.search(r'\boperation=(\d+)', result['data'])
                if not match:
                    raise RuntimeError('send admission lacked operation ID; outcome uncertain')
                operation = match[1]
                for _ in range(12):
                    command(f'ANT OPERATION {operation}', ('OK', 'UNAVAILABLE'))
                    window(.25)
            phase = 'final'
            for text in ('ANT', 'ANT CAPABILITIES', 'BATTERY', 'INPUT'):
                command(text)
        except BaseException as error:
            report['error'] = str(error)
            raise
        finally:
            if scan_attempted and not stop_attempted:
                phase = 'cleanup'
                stop_attempted = True
                try:
                    command('ANT STOP', ('ACCEPTED', 'STATE'))
                except Exception as error:
                    report['cleanup_error'] = str(error)
            report['measurements'] = summarize(samples)
            (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', required=True)
    parser.add_argument('--output', type=Path, required=True, help='new directory under repository .local/')
    parser.add_argument('--types', type=int, nargs='+', required=True, help='already selected owned sensor types')
    parser.add_argument('--seconds', type=int, default=10, help='baseline and post-stop seconds, 1..60')
    parser.add_argument('--scan-seconds', type=int, default=0, help='explicit bounded discovery, 0..60')
    parser.add_argument('--send', type=send_command, help='single owned-sensor information request: "type number transmission generation hex16"')
    parser.add_argument('--fixture-notes', required=True, help='device revision, build modes, fixtures and active coexistence traffic')
    args = parser.parse_args()
    output = args.output.resolve()
    if not output.is_relative_to((ROOT / '.local').resolve()) or output == (ROOT / '.local').resolve():
        parser.error('output must be a new directory under .local/')
    if not 1 <= args.seconds <= 60 or not 0 <= args.scan_seconds <= 60:
        parser.error('duration out of bounds')
    if not 1 <= len(args.types) <= 10 or len(set(args.types)) != len(args.types) or any(not 0 < kind <= 255 for kind in args.types):
        parser.error('supply one to ten distinct sensor types, each 1..255')
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    metadata = dict(source_revision=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
                    source_dirty=bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT)),
                    fixture_notes=args.fixture_notes, types=args.types, seconds=args.seconds,
                    scan_seconds=args.scan_seconds, send=args.send)
    (output / 'metadata.json').write_text(json.dumps(metadata, indent=2) + '\n')
    with UsbConnection(args.port, output / 'usb.bin') as connection:
        capture(connection, output, args.types, args.seconds, args.scan_seconds, args.send)
    print(f'Private evidence: {output}')


if __name__ == '__main__':
    main()
