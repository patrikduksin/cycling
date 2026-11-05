"""cycling flasher info|read|verify|reboot: the Flasher's verbs on any ESP32 esptool detects.

The verbs are the same on a local tty, through esptool with --chip auto, and
on the bridged CYCLING_PORT, through the visitor's page. There is no write:
writes stay in the `cycling flash` flows, which own what may go where. Each
verb ends with a reboot, so the device is back in its app afterwards.

    cycling flasher info                    the chip as JSON, the MAC as a sha256 prefix
    cycling flasher read ADDR SIZE PATH     SIZE bytes at ADDR into PATH, which must be under .local
    cycling flasher verify ADDR PATH        whether flash at ADDR holds PATH; exit 1 when it differs
    cycling flasher reboot [--seconds N]    reboot and keep N seconds of boot output under .local
"""

import argparse
import hashlib
import json
import sys
import time
from pathlib import Path

from cycling_devtools.device.firmware import flasher_for
from cycling_devtools.device.flasher import Chip
from cycling_devtools.device.port import add_port_argument, select_port, selected_port
from cycling_devtools.terminal.usb import acquire_lock
from cycling_devtools.workspace import DEVICE_LOCAL, private_path

LOG_DIR = DEVICE_LOCAL / 'flasher'
BOOT_SECONDS = 3
# Tells devices apart in output an agent may quote. Not anonymous: a known vendor prefix leaves 2**24 MACs to try.
MAC_DIGITS = 12


def describe(chip: Chip):
    """The chip as `info` prints it; a sha256 prefix of the MAC stands in for the MAC."""
    return {
        'mac_sha256': hashlib.sha256(chip.identity.encode()).hexdigest()[:MAC_DIGITS],
        'family': chip.family,
        'model': chip.model,
        'revision': chip.revision,
        'features': list(chip.features),
        'crystal_mhz': chip.crystal_mhz,
        'flash': {
            'manufacturer': f'{chip.flash.manufacturer:02x}',
            'device': f'{chip.flash.device:04x}',
            'size': chip.flash.size,
        },
        'usb': None
        if chip.usb is None
        else {'vendor_id': f'{chip.usb.vendor_id:04x}', 'product_id': f'{chip.usb.product_id:04x}'},
        'security': None
        if chip.security is None
        else {'secure_boot': chip.security.secure_boot, 'flash_encryption': chip.security.flash_encryption},
        'accepts_unsigned': chip.accepts_unsigned,
    }


def number(text):
    return int(text, 0)


def parser():
    parser = argparse.ArgumentParser(
        prog='cycling flasher', description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    add_port_argument(parser)
    verbs = parser.add_subparsers(dest='verb', required=True)
    verbs.add_parser('info', help='print the chip description as JSON')
    read = verbs.add_parser('read', help='read flash into a file under .local')
    read.add_argument('address', type=number)
    read.add_argument('size', type=number)
    read.add_argument('path', type=Path)
    verify = verbs.add_parser('verify', help="compare flash with a file by the chip's MD5")
    verify.add_argument('address', type=number)
    verify.add_argument('path', type=Path)
    reboot = verbs.add_parser('reboot', help='reboot into the app and keep the boot output')
    reboot.add_argument('--seconds', type=float, default=BOOT_SECONDS)
    return parser


def main(argv=None):
    arguments = parser()
    args = arguments.parse_args(argv)
    # Everything checkable offline is checked before the device is touched.
    if args.verb == 'read':
        try:
            target = private_path(args.path)
        except ValueError as error:
            arguments.error(str(error))
    if args.verb == 'verify' and not args.path.is_file():
        arguments.error(f'{args.path} is not a file')
    port = selected_port(arguments, args.port)
    LOG_DIR.mkdir(parents=True, exist_ok=True, mode=0o700)
    flasher = flasher_for(port, lambda: select_port(args.port), LOG_DIR, chip='auto')
    chip = flasher.enter()
    status = 0
    if args.verb == 'info':
        print(json.dumps(describe(chip), indent=2))
    elif args.verb == 'read':
        target.parent.mkdir(parents=True, exist_ok=True)
        flasher.read(args.address, args.size, target)
        print(f'{args.size:#x} bytes at {args.address:#x} in {target}')
    elif args.verb == 'verify':
        matched = flasher.verify(args.address, args.path)
        print(f'{args.path} {"matches" if matched else "differs from"} flash at {args.address:#x}')
        status = 0 if matched else 1
    captured = flasher.reboot(args.seconds if args.verb == 'reboot' else 0)
    if args.verb == 'reboot':
        if captured is None:
            print('Rebooted; the port did not come back for a boot capture.')
        else:
            log = LOG_DIR / f'{time.time_ns()}-boot.log'
            log.write_bytes(captured)
            print(f'Rebooted; {len(captured)} bytes of boot output in {log}')
    return status


def run_locked(argv=None):
    """One device task at a time, as the firmware tasks take the same lock."""
    try:
        lock = acquire_lock()
    except BlockingIOError:
        raise RuntimeError('Another cycling device task is using USB; wait until it finishes') from None
    with lock:
        return main(argv)


if __name__ == '__main__':
    try:
        sys.exit(run_locked())
    except (RuntimeError, OSError) as error:
        sys.exit(str(error))
