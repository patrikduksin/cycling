"""Save the next frame the C606 (or the simulator) pushes to its panel as a PNG.

The harness copies every pixel the shell hands to the display driver, after
scaling, into composition-owned capture memory while the frame renders. This
tool reads that frame back in bounded, CRC-checked chunks; it never re-renders.
Optional button presses use the harness's synthetic input before the capture.
Requires a CYCLING_HARNESS=1 build. Evidence (command trace, USB log) stays in .local/.
"""

import argparse
import json
import sys
import time
from pathlib import Path

from cycling_devtools.device.port import add_port_argument, selected_port
from cycling_devtools.harness.runner import Runner, png_rgb565
from cycling_devtools.harness.transport import Real, Virtual
from cycling_devtools.workspace import ROOT, private_path

BUTTONS = {'top-left': 0, 'bottom-left': 1, 'bottom-right': 2, 'center': 3}


def button(value):
    if value in BUTTONS:
        return BUTTONS[value]
    if value.isdigit() and int(value) in BUTTONS.values():
        return int(value)
    raise argparse.ArgumentTypeError(f'button must be one of {", ".join(BUTTONS)} or 0..3')


def screenshot(runner, output, scale=1, presses=(), settle_ms=500):
    """Press buttons, capture one frame and write it; return a JSON-ready summary."""
    started = time.monotonic()
    try:
        caps = runner.command('HARNESS CAPS')
    except RuntimeError as error:
        if 'BUSY' in str(error):
            raise RuntimeError(
                'device is in a power transition (charging standby after reset?); inspect `mise run terminal -- POWER`'
            ) from None
        raise
    if caps.get('capture') != 'supported':
        raise RuntimeError(f'frame capture is {caps.get("capture", "unsupported")}; flash a CYCLING_HARNESS=1 build')
    runner.open_session()
    try:
        if presses:
            # A press while dimmed only wakes the screen; wake first so every press navigates.
            runner.command('ACTIVITY')
            runner.delay(settle_ms / 1000)
        for index in presses:
            if str(index) not in runner.caps['buttons'].split(','):
                raise ValueError(f'button {index} is not advertised by this device')
            runner.inject({'op': 'input', 'gesture': 'button', 'button': index})
            runner.delay(settle_ms / 1000)
        requested = time.monotonic()
        runner.operation('CAPTURE START 1 100', ('ACCEPTED',))
        # A frame is captured when the owner next draws; static screens redraw at least every second.
        status = runner.wait('CAPTURE STATUS', {'state': 'completed'}, 10, harness=True)
        frame = int(status['frames'].strip(','))
        meta = runner.operation(f'CAPTURE META {frame}')
        if meta['complete'] != 'true' or meta['submitted'] != 'true' or meta['format'] != 'RGB565LE':
            raise RuntimeError('captured frame is incomplete or its display submission failed')
        width, height = int(meta['width']), int(meta['height'])
        if int(meta['bytes']) != width * height * 2:
            raise ValueError('invalid capture geometry/size')
        transfer = time.monotonic()
        before = runner.commands
        data = runner.read_frame(frame, meta)
        reads = runner.commands - before
        done = time.monotonic()
    finally:
        if runner.session:
            runner.operation('CLOSE')
            runner.session = None
    output.parent.mkdir(parents=True, exist_ok=True)
    png_rgb565(output, data, width, height, scale)
    return {
        'status': 'pass',
        'path': str(output),
        'width': width * scale,
        'height': height * scale,
        'scale': scale,
        'frame': frame,
        'crc32': meta['crc32'],
        'render_ms': int(meta['ended_ms']) - int(meta['started_ms']),
        'wait_s': round(transfer - requested, 3),
        'transfer_s': round(done - transfer, 3),
        'reads': reads,
        'elapsed_s': round(time.monotonic() - started, 3),
        'physical_panel_verified': False,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('output', nargs='?', type=Path, help='PNG path (default .local/screenshots/<time>.png)')
    parser.add_argument('--scale', type=int, default=1, choices=range(1, 5), help='nearest-neighbour upscale')
    parser.add_argument(
        '--press',
        type=button,
        action='append',
        default=[],
        help=f'synthetic button press before capture, repeatable, in order: {", ".join(BUTTONS)} or 0..3',
    )
    parser.add_argument('--settle-ms', type=int, default=500, help='wait after each press (VANA debounces 350 ms)')
    parser.add_argument('--backend', choices=['real', 'virtual'], default='real')
    parser.add_argument('--sdk', action='store_true', help='virtual backend: include VANA cycling workflows')
    add_port_argument(parser)
    args = parser.parse_args()
    stamp = time.strftime('%Y%m%d-%H%M%S')
    evidence = private_path(ROOT / '.local/screenshots' / f'{stamp}-{time.time_ns() % 1_000_000:06}')
    output = (args.output or evidence.with_suffix('.png')).resolve()
    evidence.mkdir(parents=True, mode=0o700)
    if args.backend == 'virtual':
        transport = Virtual(evidence, sdk=args.sdk)
    else:
        transport = Real(selected_port(parser, args.port), evidence / 'usb.log')
    try:
        with transport:
            runner = Runner(transport, evidence)
            try:
                result = screenshot(runner, output, args.scale, args.press, args.settle_ms)
            finally:
                runner.trace.close()
    except (OSError, ValueError, RuntimeError, TimeoutError) as error:
        print(json.dumps({'status': 'fail', 'error': str(error), 'evidence': str(evidence)}))
        return 1
    print(json.dumps(result | {'evidence': str(evidence)}))
    return 0


if __name__ == '__main__':
    sys.exit(main())
