"""Harness transports retain one owner across close/reopen and firmware restart."""
import json
import fcntl
import os
from pathlib import Path
import secrets
import select
import subprocess
import time

from cycling_devtools.terminal.usb import ROOT, UsbConnection, open_no_reset, terminal_reply


class ManualAction(RuntimeError):
    pass


def usb_node(port):
    node = Path('/sys/class/tty') / Path(port).name / 'device'
    for parent in node.resolve().parents:
        if (parent / 'idVendor').exists():
            return parent
    raise ValueError('cannot find USB device ancestor')


def reset_device(port, identity):
    node = usb_node(port)
    if usb_identity(port) != identity:
        raise ValueError('USB reset target identity changed')
    bus = int((node / 'busnum').read_text())
    device = int((node / 'devnum').read_text())
    path = f'/dev/bus/usb/{bus:03}/{device:03}'
    fd = os.open(path, os.O_WRONLY)
    try:
        # Linux usbdevice_fs.h: USBDEVFS_RESET = _IO('U', 20). Targets this device, not its hub.
        fcntl.ioctl(fd, 0x5514, 0)
    finally:
        os.close(fd)


def usb_identity(port):
    node = Path('/sys/class/tty') / Path(port).name / 'device'
    for parent in node.resolve().parents:
        if (parent / 'idVendor').exists():
            return {key: (parent / key).read_text().strip() for key in ('idVendor', 'idProduct', 'serial')}
    raise ValueError('cannot establish USB device identity for reconnect')


def rediscover(identity):
    matches = []
    for node in Path('/sys/class/tty').glob('ttyACM*'):
        port = '/dev/' + node.name
        try:
            if usb_identity(port) == identity:
                matches.append(port)
        except (OSError, ValueError):
            continue
    if len(matches) == 1:
        return matches[0]
    raise OSError('intended USB device is absent or ambiguous')


class Real(UsbConnection):
    virtual = False

    def terminal_command(self, command, timeout=8):
        self.request_count = getattr(self, 'request_count', 0) + 1
        return super().terminal_command(command, timeout)

    def __enter__(self):
        self.identity = usb_identity(self.port)
        super().__enter__()
        self.request_id = secrets.randbelow(1 << 30) + 1
        # Startup can leave a partial line. Inspect with fresh read-only requests;
        # no mutation is sent or replayed during this attachment handshake.
        stop = time.monotonic() + 8
        while time.monotonic() < stop:
            try:
                if self.terminal_command('INFO', timeout=2)['status'] == 'OK':
                    return self
            except (OSError, RuntimeError, TimeoutError):
                self.boot = [None]
            time.sleep(.1)
        self.__exit__(None, None, None)
        raise ManualAction('C606 terminal did not become ready. Check its screen and reconnect its cable; then rerun read-only inspection.')

    def delay(self, seconds):
        time.sleep(seconds)

    def recover(self, mode, missing_seconds=1, timeout=25):
        if mode not in ('terminal', 'restart', 'usb-reset'):
            raise ValueError('supported recovery modes: terminal, restart, usb-reset; VBUS removal requires physical action')
        previous_boot = self.boot[0]
        started = time.monotonic()
        uncertain = False
        if mode == 'restart':
            try:
                reply = self.terminal_command('RESTART')
                if reply['status'] != 'OK' and reply['status'] != 'ACCEPTED':
                    raise ValueError('firmware rejected restart')
            except (OSError, RuntimeError, TimeoutError):
                uncertain = True  # Inspect after reconnect. Never resend RESTART.
        os.close(self.fd)
        self.fd = None
        if mode == 'usb-reset':
            reset_device(self.port, self.identity)
        self.pending.clear()
        self.delay(missing_seconds)
        stop = time.monotonic() + timeout
        while time.monotonic() < stop:
            try:
                self.port = rediscover(self.identity)
                self.fd = open_no_reset(self.port)
                self.boot = [None]
                reply = self.terminal_command('INFO', timeout=min(3, max(.1, stop - time.monotonic())))
                if reply['status'] != 'OK':
                    raise RuntimeError('INFO not ready')
                # The session layer additionally verifies the boot token via CAPS.
                return {'mode': mode, 'elapsed_s': time.monotonic() - started,
                        'restart_reply_uncertain': uncertain, 'previous_log_boot': previous_boot,
                        'vbus_removed': False, 'info': reply}
            except (OSError, RuntimeError, TimeoutError):
                if self.fd is not None:
                    os.close(self.fd)
                    self.fd = None
                self.pending.clear()
                time.sleep(.2)
        raise ManualAction('Reconnect the C606 cable, press its power button if needed, then run a new read-only inspection. No mutation was replayed.')


class Virtual:
    virtual = True

    def __init__(self, directory, state_dir=None, sdk=False, harness=True):
        self.directory, self.state_dir = Path(directory), Path(state_dir or Path(directory) / 'state')
        self.features = ','.join(name for name, enabled in (('debug-harness', harness), ('cycling', sdk)) if enabled)
        self.request_id = secrets.randbelow(1 << 30) + 1
        self.boot, self.pending = [None], bytearray()
        self.process = self.log = self.errors = None
        self.request_count = self.fixture_count = 0

    def __enter__(self):
        self.state_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.log = (self.directory / 'virtual.log').open('ab')
        self.errors = (self.directory / 'virtual-stderr.log').open('ab')
        self.process = subprocess.Popen(['cargo', 'run', '-p', 'shell-simulator', '--quiet', '--locked', '--no-default-features',
            '--features', self.features, '--bin', 'shell-simulator', '--', '--terminal',
            '--state-dir', str(self.state_dir)], cwd=ROOT, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=self.errors, bufsize=0)
        return self

    def __exit__(self, *_):
        if self.process:
            self.process.stdin.close()
            try:
                self.process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.process.terminate()
                try:
                    self.process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait(timeout=2)
            self.process.stdout.close()
        if self.log:
            self.log.close()
        if self.errors:
            self.errors.close()

    def terminal_command(self, command, timeout=30):
        self.request_count += 1
        self.request_id += 1
        self.process.stdin.write(f'CMD {self.request_id} {command}\n'.encode('ascii'))
        stop = time.monotonic() + timeout
        while time.monotonic() < stop:
            if not select.select([self.process.stdout], [], [], min(.2, max(0, stop - time.monotonic())))[0]:
                continue
            data = os.read(self.process.stdout.fileno(), 65536)
            if not data:
                raise RuntimeError('virtual backend disconnected; mutation outcome uncertain')
            self.log.write(data)
            self.log.flush()
            reply = terminal_reply(self.pending, data, self.request_id, self.boot)
            if reply is not None:
                return reply
        raise TimeoutError('virtual reply timeout; mutation outcome uncertain')

    def delay(self, seconds):
        self.fixture(f'ADVANCE {max(1, round(seconds * 1000))}')

    def fixture(self, command):
        self.fixture_count += 1
        if any(c in command for c in '\r\n\x00') or len(command) > 120:
            raise ValueError('invalid fixture framing')
        self.process.stdin.write(f'FIXTURE {command}\n'.encode('ascii'))
        stop = time.monotonic() + 10
        while time.monotonic() < stop:
            while b'\n' in self.pending:
                line, _, rest = self.pending.partition(b'\n')
                self.pending[:] = rest
                result = json.loads(line)
                if result.get('type') == 'fixture':
                    return result
            if not select.select([self.process.stdout], [], [], .2)[0]:
                continue
            data = os.read(self.process.stdout.fileno(), 65536)
            if not data:
                raise RuntimeError('virtual fixture stream disconnected')
            self.log.write(data)
            self.log.flush()
            self.pending.extend(data)
            if len(self.pending) > 4096:
                raise ValueError('oversized fixture reply')
        raise TimeoutError('fixture reply timeout')

    def recover(self, mode, missing_seconds=1, timeout=25):
        if mode not in ('terminal', 'restart'):
            raise ValueError('physical USB recovery unsupported by virtual device')
        start = time.monotonic()
        if mode == 'restart':
            result = self.terminal_command('RESTART')
            if result['status'] not in ('OK', 'ACCEPTED'):
                raise ValueError('virtual restart rejected')
        else:
            self.delay(missing_seconds)
        self.boot = [None]
        return {'mode': mode, 'elapsed_s': time.monotonic() - start, 'vbus_removed': False,
                'virtual': True, 'info': self.terminal_command('INFO')}
