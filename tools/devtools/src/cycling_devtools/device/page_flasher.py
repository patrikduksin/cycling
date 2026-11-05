"""The visitor's page as a Flasher: each verb is one op esptool-js runs next to the chip.

The bridge (`cycling remote`, web/agent) exports CYCLING_FLASHER, a unix
socket that speaks JSON lines, one request and one reply. The agent reads
and writes the files named here, checks every byte against the chip's own
digest and resends an op the relay dropped, so this side only asks and
waits. One connection is one lease: closing it gives the device back.
"""

import json
import socket
import time
from pathlib import Path

from cycling_devtools.device.flasher import Chip, Flash, FlasherError, Security, Usb


class PageFlasher:
    def __init__(self, door: Path, log_dir: Path):
        self.door, self.log_dir = door, log_dir
        self.connection: socket.socket | None = None
        self.replies = None

    def _ask(self, request):
        if self.connection is None:
            connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            try:
                connection.connect(str(self.door))
            except OSError as error:
                connection.close()
                raise FlasherError(f'the bridge device socket is not answering ({error.strerror})') from None
            self.connection, self.replies = connection, connection.makefile('rb')
        self.connection.sendall(json.dumps(request).encode() + b'\n')
        line = self.replies.readline()
        # The private log keeps both sides; the enter reply holds the identity.
        with (self.log_dir / f'{time.time_ns()}-page.log').open('wb') as log:
            log.write(json.dumps(request).encode() + b'\n' + line)
        if not line:
            raise FlasherError('the bridge closed the device socket')
        reply = json.loads(line)
        if 'error' in reply:
            error = reply['error']
            detail = f': {error["detail"]}' if error.get('detail') else ''
            raise FlasherError(f'{request["op"]} through the page failed, {error["reason"]}{detail}')
        return reply['ok']

    def enter(self):
        chip = self._ask({'op': 'enter'})
        mac, flash, usb, security = chip['mac'], chip['flash'], chip['usb'], chip['security']
        return Chip(
            identity=':'.join(mac[index : index + 2] for index in range(0, 12, 2)),
            family=chip['family'],
            model=chip['model'],
            revision=chip['revision'],
            features=tuple(chip['features']),
            crystal_mhz=chip['crystal_mhz'],
            flash=Flash(flash['manufacturer'], flash['device'], flash['size']),
            usb=None if usb is None else Usb(usb['vendor_id'], usb['product_id']),
            security=None if security is None else Security(security['secure_boot'], security['flash_encryption']),
        )

    def read(self, address, size, path):
        self._ask({'op': 'read', 'address': address, 'size': size, 'path': str(Path(path).resolve())})

    def write(self, address, path):
        self._ask({'op': 'write', 'address': address, 'path': str(Path(path).resolve())})

    def verify(self, address, path):
        return self._ask({'op': 'verify', 'address': address, 'path': str(Path(path).resolve())})['matched']

    def reboot(self, seconds):
        capture = (self.log_dir / f'{time.time_ns()}-page-capture.bin').resolve()
        if not self._ask({'op': 'reboot', 'seconds': seconds, 'path': str(capture)})['captured']:
            return None
        captured = capture.read_bytes()
        capture.unlink()
        return captured
