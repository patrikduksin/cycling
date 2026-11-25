"""Pair a phone with the C606: make a fresh link key over USB and show it as a QR code.

Sends `PHONE PAIR`, which replaces any earlier key (the old phone stops
working), waits until `PHONE` reports the new pairing saved, then writes the
vanaride:// URL to .local/pairing/vana.url in the primary checkout, where
`cycling ble-http` reads it, and prints the URL and its QR code. Scan the QR
with the iPhone Camera, or paste the URL into the VANA app. Run it through
`cycling device pair` on the device host.

    cycling pair            # pair again and show the new QR code
    cycling pair --show     # show the saved pairing without touching the device
"""

import argparse
import json
import os
import re
import sys
import time
from pathlib import Path

from cycling_devtools.connectivity.qr import encode, render
from cycling_devtools.connectivity.vana_link import PAIRING_FILE, parse_pairing
from cycling_devtools.device.port import add_port_argument, selected_port
from cycling_devtools.terminal.usb import UsbConnection
from cycling_devtools.workspace import ROOT

SAVE_WAIT_S = 5
POLL_S = 0.25
BUSY_TRIES = 20


def reply_pairing(text):
    """The pairing in a `PHONE PAIR` reply: `id=<hex8> url=vanaride://pair?...`."""
    url = re.search(r'url=(\S+)', text)
    stated = re.search(r'\bid=([0-9a-f]{8})\b', text)
    if not url or not stated:
        raise RuntimeError(f'PHONE PAIR reply has no id and url: {text!r}')
    pairing = parse_pairing(url.group(1))
    if pairing.id_hex != stated.group(1):
        raise RuntimeError(f'PHONE PAIR reply names id {stated.group(1)} but its URL holds {pairing.id_hex}')
    return pairing


def paired_state(text):
    """`PHONE`'s `paired=` field: an id in hex, `pending` or `none`."""
    found = re.search(r'\bpaired=(\S+)', text)
    if not found:
        raise RuntimeError(f'PHONE reply has no paired= field: {text!r}')
    return found.group(1)


def pair(connection, sleep=time.sleep, clock=time.monotonic):
    for _ in range(BUSY_TRIES):
        reply = connection.terminal_command('PHONE PAIR')
        if reply['status'] != 'BUSY':
            break
        sleep(0.5)
    if reply['status'] not in ('OK', 'ACCEPTED'):
        raise RuntimeError(f'PHONE PAIR answered {reply["status"]}: {reply.get("data", "")}')
    pairing = reply_pairing(str(reply.get('data', '')))
    deadline = clock() + SAVE_WAIT_S
    while True:
        reply = connection.terminal_command('PHONE')
        state = paired_state(str(reply.get('data', '')))
        if state == pairing.id_hex:
            return pairing
        if state == 'none':
            raise RuntimeError('the device did not save the pairing: PHONE reports paired=none')
        if state != 'pending':
            raise RuntimeError(f'PHONE reports paired={state}, not the new id {pairing.id_hex}')
        if clock() >= deadline:
            raise TimeoutError(f'pairing {pairing.id_hex} still pending after {SAVE_WAIT_S} s')
        sleep(POLL_S)


def save(path, url):
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    path.parent.chmod(0o700)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, 'w') as file:
        os.fchmod(file.fileno(), 0o600)
        file.write(url + '\n')


def show(url, invert):
    print(url)
    print(render(encode(url), invert=invert))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    add_port_argument(parser)
    parser.add_argument('--show', action='store_true', help='print the saved pairing again without re-pairing')
    parser.add_argument('--invert', action='store_true', help='draw dark modules filled, for a light terminal')
    parser.add_argument('--output', type=Path, default=ROOT / '.local/terminal' / str(time.time_ns()))
    args = parser.parse_args()
    if args.show:
        try:
            url = parse_pairing(PAIRING_FILE.read_text()).url()
        except (OSError, ValueError) as error:
            print(f'no saved pairing at {PAIRING_FILE}: {error}', file=sys.stderr)
            return 1
        show(url, args.invert)
        return 0
    port = selected_port(parser, args.port)
    args.output.mkdir(parents=True, exist_ok=False, mode=0o700)
    try:
        with UsbConnection(port, args.output / 'usb.log') as connection:
            pairing = pair(connection)
    except (OSError, RuntimeError, ValueError, TimeoutError) as error:
        print(str(error), file=sys.stderr)
        return 1
    save(PAIRING_FILE, pairing.url())
    print(json.dumps({'id': pairing.id_hex, 'saved': str(PAIRING_FILE)}))
    show(pairing.url(), args.invert)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
