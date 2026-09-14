"""Timed C606 physical bench capture. Read-only commands; raw evidence stays private."""
import argparse
import json
import os
from pathlib import Path
import sys
import time
from usb import ROOT, UsbConnection
from device_port import DiscoveryError, resolve_port

STEPS = (
    (0, 'Leave the device still, screen facing up, for the quiet baseline.'),
    (5, 'Click top-left once, release, wait one second, then click twice with a one-second gap.'),
    (15, 'Repeat that single-click then two-click sequence on bottom-left.'),
    (25, 'Repeat that single-click then two-click sequence on bottom-right.'),
    (35, 'Hold bottom-left for two seconds, release, then wait.'),
    (43, 'Hold bottom-right for two seconds, release, then wait.'),
    (51, 'Hold bottom-left and bottom-right together for two seconds, then release both.'),
    (60, 'Keep screen up and still for five seconds.'),
    (65, 'Rotate onto its left edge and hold still for five seconds.'),
    (70, 'Rotate onto its right edge and hold still for five seconds.'),
    (75, 'Rotate onto its top edge and hold still for five seconds.'),
    (80, 'Rotate onto its bottom edge and hold still for five seconds.'),
    (85, 'Turn screen down and hold still for five seconds.'),
    (90, 'Return screen up. Slowly rotate, lift and lower the device for fifteen seconds without pulling the USB cable.'),
    (105, 'Hold top-left for two seconds then release. The companion may restart independently; watch the screen.'),
    (115, 'If it remains responsive, hold top-left for five seconds then release. If it turns off, use the normal power button to start it; reconnect USB only if needed.'),
    (130, 'Leave it still and connected. Wait for the capture to finish at 150 seconds.'),
)
COMMANDS = ('INPUT', 'MOTION', 'PRESSURE', 'BATTERY', 'POSITION', 'GNSS', 'STATUS')


def collect(directory, port, seconds=150):
    directory = directory.resolve()
    if not directory.is_relative_to((ROOT / '.local').resolve()) or directory == (ROOT / '.local').resolve():
        raise ValueError('output must be a new directory under ignored .local/')
    directory.mkdir(parents=True, exist_ok=False, mode=0o700)
    start = time.monotonic()
    next_step = 0
    connection = None
    attempt = 0
    count = gaps = 0
    interrupted = False
    with (directory / 'observations.jsonl').open('x') as output:
        try:
            while time.monotonic() - start < seconds:
                elapsed = time.monotonic() - start
                while next_step < len(STEPS) and elapsed >= STEPS[next_step][0]:
                    print(f'{STEPS[next_step][0]:3}s: {STEPS[next_step][1]}', file=sys.stderr, flush=True)
                    output.write(json.dumps(dict(type='instruction', elapsed_s=elapsed, action=STEPS[next_step][1]))+'\n')
                    next_step += 1
                if connection is None:
                    attempt += 1
                    try:
                        verified = resolve_port()
                        candidate_port = Path(port) if attempt == 1 else verified
                        if candidate_port.resolve() != verified.resolve():
                            raise DiscoveryError('requested port does not match the backed-up device')
                        candidate = UsbConnection(candidate_port, directory / f'usb-{attempt}.log')
                        candidate.__enter__()
                        connection = candidate
                    except (OSError, RuntimeError, TimeoutError, DiscoveryError):
                        # Opening/handshake is read-only. Do not replay mutations.
                        output.write(json.dumps(dict(type='gap', elapsed_s=elapsed, reason='USB unavailable during physical sequence'))+'\n')
                        output.flush()
                        gaps += 1
                        time.sleep(.5)
                        continue
                command = COMMANDS[count % len(COMMANDS)]
                try:
                    reply = connection.terminal_command(command, timeout=2)
                    output.write(json.dumps(dict(type='sample', elapsed_s=elapsed, command=command, reply=reply))+'\n')
                    count += 1
                except (OSError, RuntimeError, TimeoutError, ValueError) as error:
                    output.write(json.dumps(dict(type='gap', elapsed_s=elapsed, reason=type(error).__name__))+'\n')
                    gaps += 1
                    connection.__exit__(None, None, None)
                    connection = None
                output.flush()
                time.sleep(.1)
        except KeyboardInterrupt:
            interrupted = True
            output.write(json.dumps(dict(type='interrupted', elapsed_s=time.monotonic() - start))+'\n')
            output.flush()
        finally:
            if connection is not None:
                connection.__exit__(None, None, None)
    result = dict(status='interrupted' if interrupted else 'captured', elapsed_s=time.monotonic() - start, samples=count, transport_gaps=gaps, report=str(directory), physical_acceptance='requires inspection of observations and owner actions')
    (directory/'summary.json').write_text(json.dumps(result, indent=2)+'\n')
    print(json.dumps(result))
    return 130 if interrupted else 0


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path, nargs='?')
    parser.add_argument('--port', default=os.environ.get('CYCLING_PORT','/dev/ttyACM0'))
    parser.add_argument('--plan', action='store_true', help='print sequence without opening USB')
    args=parser.parse_args()
    if args.plan:
        for second, action in STEPS: print(f'{second:3}s: {action}')
        return 0
    if args.output is None: parser.error('a new private output directory is required')
    try: return collect(args.output,args.port)
    except KeyboardInterrupt: return 130
    except (OSError,RuntimeError,ValueError) as error:
        print(str(error),file=sys.stderr);return 1

if __name__=='__main__': raise SystemExit(main())
