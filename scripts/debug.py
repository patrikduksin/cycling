"""C606 USB test sessions, input injection, recordings and scenario evidence."""
import argparse
import fcntl
import html
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import termios
import time
import tty

from screenshot import ROOT, checksum, png

FRAME_LOG = b'CYCLING_FRAME frame='


def torn_frame_prefix(prefix):
    """Recognize a truncated ordinary frame line without accepting arbitrary junk."""
    if not prefix:
        return False
    if FRAME_LOG.startswith(prefix):
        return True
    if not prefix.startswith(FRAME_LOG):
        return False
    remainder = prefix[len(FRAME_LOG):]
    for field in [b' render_ms=', b' draws=', b' skipped=']:
        digits = len(remainder) - len(remainder.lstrip(b'0123456789'))
        remainder = remainder[digits:]
        if field.startswith(remainder):
            return True
        if not remainder.startswith(field):
            return False
        remainder = remainder[len(field):]
    return not remainder or remainder.isdigit()


class Recording:
    """Reject incomplete delta streams rather than making misleading videos."""
    def __init__(self):
        self.stream = None
        self.sequence = 0
        self.pixels = [0] * 8480
        self.pending = None
        self.stopped = set()

    def feed(self, line):
        f = line.split()
        if not f or f[0] != b'CYCLING_REC':
            return None
        if len(f) == 8 and f[1] == b'BEGIN':
            stream, seq, frame, ms, key = map(int, f[2:7])
            if self.pending is not None:
                raise ValueError('Missing recording END')
            if stream != self.stream:
                if seq != 0 or key != 1:
                    raise ValueError('Recording lacks its initial key frame')
                self.stream, self.sequence = stream, 0
                self.pixels = [0] * 8480
            if seq != self.sequence or key != int(seq == 0):
                raise ValueError('Dropped or unexpected recording frame')
            self.pending = dict(stream=stream, sequence=seq, frame=frame, ms=ms,
                                checksum=int(f[7], 16), key=key)
            self.rows = set()
        elif len(f) == 6 and f[1] == b'ROW':
            if self.pending is None or (int(f[2]), int(f[3])) != (self.stream, self.sequence):
                raise ValueError('Recording row outside its frame')
            row, encoded = int(f[4]), f[5]
            if not 0 <= row < 106 or row in self.rows or len(encoded) % 6:
                raise ValueError('Invalid recording row')
            values = []
            for i in range(0, len(encoded), 6):
                count, color = int(encoded[i:i + 2], 16), int(encoded[i + 2:i + 6], 16)
                if count == 0 or len(values) + count > 80:
                    raise ValueError('Invalid pixel run')
                values.extend([color] * count)
            if len(values) != 80:
                raise ValueError('Truncated recording row')
            self.pixels[row * 80:(row + 1) * 80] = values
            self.rows.add(row)
        elif len(f) == 4 and f[1] == b'END':
            if self.pending is None or (int(f[2]), int(f[3])) != (self.stream, self.sequence):
                raise ValueError('Unexpected recording END')
            if self.pending['key'] and len(self.rows) != 106:
                raise ValueError('Incomplete key frame')
            if checksum(self.pixels) != self.pending['checksum']:
                raise ValueError('Recording checksum mismatch')
            result = self.pending, self.pixels.copy()
            self.pending = None
            self.sequence += 1
            return result
        elif len(f) == 4 and f[1] == b'STOP':
            if self.pending is not None or (int(f[2]), int(f[3])) != (self.stream, self.sequence):
                raise ValueError('Recording ended with missing frames')
            self.stopped.add(self.stream)
        else:
            raise ValueError('Malformed recording message')
        return None


class Device:
    def __init__(self, port, directory):
        self.directory = Path(directory)
        self.directory.mkdir(parents=True, exist_ok=False, mode=0o700)
        self.fd = None
        self.lock = None
        self.log = None
        self.active = False
        self.counter = 0
        self.replies = {}
        self.pending = bytearray()
        self.recording = Recording()
        self.frames = []
        self.events = []
        self.last_ping = time.monotonic()
        self.port = port

    def __enter__(self):
        try:
            self.lock = (ROOT / '.local/usb.lock').open('w')
            fcntl.flock(self.lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.fd = os.open(self.port, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
            tty.setraw(self.fd)
            attributes = termios.tcgetattr(self.fd)
            attributes[2] = (attributes[2] | termios.CLOCAL | termios.CREAD) & ~termios.HUPCL
            attributes[4] = attributes[5] = termios.B115200
            termios.tcsetattr(self.fd, termios.TCSANOW, attributes)
            self.log = (self.directory / 'usb.log').open('wb')
            self.baseline = self.command('BEGIN')
            self.expected_brightness = self.baseline['brightness']
            if self.baseline.get('protocol') != 1:
                raise RuntimeError('Unsupported device debug protocol')
            self.active = True
            return self
        except BaseException as error:
            self.close()
            (self.directory / 'report.json').write_text(json.dumps(dict(
                ok=False, error=str(error), events=self.events, frames=self.frames), indent=2) + '\n')
            raise

    def close(self):
        if self.fd is not None:
            os.close(self.fd)
            self.fd = None
        if self.log is not None:
            self.log.close()
        if self.lock is not None:
            self.lock.close()

    def __exit__(self, kind, error, traceback):
        cleanup_error = None
        try:
            if self.active:
                self.final = self.command('END')
                self.active = False
                if self.final['active'] or self.final['fake_battery'] or self.final['x'] != -1:
                    raise RuntimeError('Test session did not release injected state')
                if self.final['brightness'] != self.expected_brightness:
                    raise RuntimeError('Test session did not restore brightness')
                if (self.final['screen'], self.final['focus'], self.final['pressed'],
                        self.final['input_blocked']) != \
                        (self.baseline['screen'], self.baseline['focus'], -1, False):
                    raise RuntimeError('Test session did not restore navigation')
        except Exception as e:
            cleanup_error = str(e)
        finally:
            self.close()
            report = dict(ok=error is None and cleanup_error is None,
                          error=str(error) if error else cleanup_error, cleanup_error=cleanup_error,
                          baseline=getattr(self, 'baseline', None), final=getattr(self, 'final', None),
                          events=self.events, frames=self.frames)
            (self.directory / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        if error is None and cleanup_error:
            raise RuntimeError(cleanup_error)

    def send(self, command):
        self.counter += 1
        data = f'DBG {self.counter} {command}\n'.encode('ascii')
        if len(data) > 96 or b'\n' in data[:-1] or b'\r' in data:
            raise ValueError('Invalid debug command')
        if os.write(self.fd, data) != len(data):
            raise RuntimeError('Short USB command write')
        self.events.append(dict(id=self.counter, command=command, host_ms=round(time.monotonic() * 1000)))
        return self.counter

    def pump(self, seconds=0.1):
        if self.active and time.monotonic() - self.last_ping >= 1:
            self.send('PING')
            self.last_ping = time.monotonic()
        if not select.select([self.fd], [], [], max(0, seconds))[0]:
            return
        data = os.read(self.fd, 65536)
        if not data:
            raise RuntimeError('Device disconnected')
        self.log.write(data)
        self.log.flush()
        self.feed(data)

    def feed(self, data):
        """Parse USB bytes and tolerate reads split across one known torn log prefix."""
        self.pending.extend(data)
        while b'\n' in self.pending:
            line, _, self.pending = self.pending.partition(b'\n')
            if b'CYCLING_BOOT' in line:
                raise RuntimeError('Device rebooted during test session')
            if b'CYCLING_DEBUG expired' in line:
                raise RuntimeError('Device test lease expired')
            marker = line.find(b'CYCLING_DEBUG ')
            if marker > 0 and torn_frame_prefix(line[:marker]):
                line = line[marker:]
            if line.startswith(b'CYCLING_DEBUG '):
                try:
                    _, id_text, result, body = line.split(b' ', 3)
                    id = int(id_text)
                    state = json.loads(body)
                except (ValueError, json.JSONDecodeError) as error:
                    raise ValueError('Malformed debug reply') from error
                if id == 0 and result == b'INVALID':
                    raise ValueError('Firmware rejected a malformed debug command')
                self.replies[id] = result.decode(), state
                # Keep evidence including heartbeat responses, without unbounded reply storage.
                for event in reversed(self.events):
                    if event.get('id') == id:
                        event.update(result=result.decode(), state=state)
                        if event['command'] == 'PING':
                            self.replies.pop(id, None)
                        break
            frame = self.recording.feed(line)
            if frame is not None:
                metadata, pixels = frame
                self.latest_pixels = pixels
                self.latest_frame = metadata
                name = f"frame-{metadata['stream']:04d}-{metadata['sequence']:04d}.png"
                (self.directory / name).write_bytes(png(pixels))
                metadata['file'] = name
                raw_name = name.removesuffix('.png') + '.rgb565'
                (self.directory / raw_name).write_bytes(
                    b''.join(pixel.to_bytes(2, 'little') for pixel in pixels))
                metadata['raw_file'] = raw_name
                self.frames.append(metadata)
        if len(self.pending) > 8192:
            raise ValueError('Oversized USB line')

    def command(self, command, timeout=5, expected='OK'):
        id = self.send(command)
        deadline = time.monotonic() + timeout
        while id not in self.replies:
            if time.monotonic() >= deadline:
                raise TimeoutError(f'No acknowledgment for {command}')
            self.pump()
        result, state = self.replies.pop(id)
        if result != expected:
            raise RuntimeError(f'{command}: {result}')
        if command.startswith('PERSIST '):
            self.expected_brightness = state['brightness']
        self.completed = id
        return state

    def wait(self, seconds):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            self.pump(min(.1, deadline - time.monotonic()))

    def expect(self, expected, timeout=5):
        deadline = time.monotonic() + timeout
        while True:
            state = self.command('STATE')
            if all(state.get(key) == value for key, value in expected.items()):
                return state
            if time.monotonic() >= deadline:
                raise AssertionError(f'Expected {expected}, got {state}')
            self.wait(.2)

    def tap(self, x, y, seconds=.12):
        self.command(f'TOUCH {x} {y}')
        self.wait(seconds)
        return self.command('RELEASE')

    def drag(self, start, end, seconds=2, steps=20):
        if not 1 <= steps <= 200 or not 0 < seconds <= 20:
            raise ValueError('Drag requires 1..200 steps and 0..20 seconds')
        self.command(f'TOUCH {start[0]} {start[1]}')
        for i in range(1, steps + 1):
            self.wait(seconds / steps)
            x = round(start[0] + (end[0] - start[0]) * i / steps)
            y = round(start[1] + (end[1] - start[1]) * i / steps)
            self.command(f'TOUCH {x} {y}')
        return self.command('RELEASE')

    def pixel(self, x, y, color, after_ms=0, timeout=3):
        if not (0 <= x < 240 and 0 <= y < 320 and 0 <= color <= 65535):
            raise ValueError('Invalid pixel assertion')
        deadline = time.monotonic() + timeout
        while not hasattr(self, 'latest_frame') or self.latest_frame['ms'] < after_ms:
            if time.monotonic() >= deadline:
                raise TimeoutError('No recorded frame after the input acknowledgment')
            self.pump()
        value = self.latest_pixels[min(max(y - 1, 0) // 3, 105) * 80 + x // 3]
        if value != color:
            raise AssertionError(f'Pixel {x},{y}: expected {color:04x}, got {value:04x}')
        self.events.append(dict(assert_pixel=[x, y, color], frame=self.latest_frame['frame']))

    def capture(self):
        self.command('CAPTURE')
        stream = self.completed
        deadline = time.monotonic() + 5
        while stream not in self.recording.stopped:
            if time.monotonic() >= deadline:
                raise TimeoutError('Capture did not complete')
            self.pump()
        return next(f for f in self.frames if f['stream'] == stream)


def export(directory):
    """Keep raw frame timing as evidence; encode a variable-frame-rate MP4."""
    directory = Path(directory)
    report = json.loads((directory / 'report.json').read_text())
    frames = report['frames']
    if not frames:
        return
    videos = []
    # A stopped recording is a gap in observation, not a frozen screen.
    for stream in dict.fromkeys(frame['stream'] for frame in frames):
        segment = [frame for frame in frames if frame['stream'] == stream]
        manifest = ['ffconcat version 1.0']
        for i, frame in enumerate(segment):
            duration = (segment[i + 1]['ms'] - frame['ms']) / 1000 if i + 1 < len(segment) else .2
            if duration <= 0:
                raise ValueError('Recording timestamps did not advance')
            manifest.extend([f"file '{frame['file']}'", "option framerate 1000", f'duration {duration:.6f}'])
        manifest.extend([f"file '{segment[-1]['file']}'", "option framerate 1000"])
        name = f'stream-{stream:04d}'
        (directory / f'{name}.ffconcat').write_text('\n'.join(manifest) + '\n')
        if shutil.which('ffmpeg'):
            result = subprocess.run(['ffmpeg', '-hide_banner', '-loglevel', 'error', '-y',
                                     '-safe', '0', '-f', 'concat', '-i', f'{name}.ffconcat',
                                     '-fps_mode', 'vfr', '-c:v', 'libx264', '-pix_fmt', 'yuv420p',
                                     '-video_track_timescale', '1000', '-movflags', '+faststart', f'{name}.mp4'],
                                    cwd=directory, capture_output=True)
            (directory / f'{name}-ffmpeg.log').write_bytes(result.stderr)
            if result.returncode:
                raise RuntimeError('Video encoding failed; inspect ffmpeg log')
            videos.append(f'<video controls src="{name}.mp4"></video>')
    pictures = ''.join(f'<figure><img src="{html.escape(f["file"])}"><figcaption>'
                       f'Frame {f["frame"]}, {f["ms"]} ms</figcaption></figure>' for f in frames)
    (directory / 'index.html').write_text('<!doctype html><meta charset="utf-8"><title>C606 test</title>'
        '<style>body{font:16px sans-serif;background:#eee}main{display:flex;flex-wrap:wrap}'
        'figure{margin:8px}img{image-rendering:pixelated}</style>'
        f'<h1>{"Passed" if report["ok"] else "Failed"}</h1><a href="report.json">Test report</a> '
        '<a href="usb.log">USB log</a><p>' + ''.join(videos) + '</p>'
        f'<main>{pictures}</main>')


def scenario(device, steps):
    for step in steps:
        action = step['action']
        if action == 'command':
            device.command(step['value'])
        elif action == 'wait':
            device.wait(float(step['seconds']))
        elif action == 'expect':
            device.expect(step['state'], float(step.get('timeout', 5)))
        elif action == 'tap':
            device.tap(*step['point'], float(step.get('seconds', .12)))
        elif action == 'drag':
            device.drag(step['from'], step['to'], float(step.get('seconds', 2)), int(step.get('steps', 20)))
        elif action == 'capture':
            device.capture()
        elif action == 'pixel':
            device.pixel(*step['point'], int(step['rgb565'], 16))
        elif action == 'wake':
            wake_if_dimmed(device)
        else:
            raise ValueError(f'Unknown scenario action {action}')


def wake_if_dimmed(device):
    if device.command('STATE').get('dimmed'):
        device.command('TOUCH 1 1')
        device.command('RELEASE')
        device.expect({'dimmed': False, 'x': -1, 'y': -1})


def wait_increase(device, key, previous, timeout=45):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        state = device.command('STATE')
        if state[key] > previous:
            return state
        device.wait(.25)
    raise TimeoutError(f'{key} did not increase from {previous}')


def wait_below(device, key, previous, timeout=45):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        state = device.command('STATE')
        if state[key] < previous:
            return state
        device.wait(.25)
    raise TimeoutError(f'{key} did not fall below {previous}')


def wifi_recovery(device):
    baseline = device.expect({'wifi': 4, 'wifi_fault': 0, 'time_status': 'fresh'}, 45)
    state = baseline

    # A deliberate link reset must associate and verify again.
    device.command('WIFI')
    offline = device.expect({'wifi': 5, 'time_status': 'offline'}, 5)
    state = wait_increase(device, 'wifi_successes', state['wifi_successes'])
    device.expect({'wifi': 4})
    state = wait_below(device, 'time_age_ms', offline['time_age_ms'])
    assert state['utc'] >= offline['utc']
    assert state['wifi_associations'] > baseline['wifi_associations']

    # Public-server failures retry over the usable link, without another association.
    for fault in (1, 2):
        device.command(f'WIFI_FAULT {fault}')
        associations = state['wifi_associations']
        failures = state['wifi_failures']
        successes = state['wifi_successes']
        device.command('WIFI')
        failed = wait_increase(device, 'wifi_failures', failures)
        assert failed['wifi'] == 5 and failed['wifi_fault'] == fault
        device.command('WIFI_FAULT 0')
        state = wait_increase(device, 'wifi_successes', successes)
        assert state['wifi_associations'] == failed['wifi_associations']
        assert failed['wifi_associations'] > associations
        device.expect({'wifi': 4, 'wifi_fault': 0})

    final = device.command('STATE')
    assert final['heap_free'] >= baseline['heap_free'] - 4096
    assert final['bad_crc'] == baseline['bad_crc']
    assert final['uart_errors'] == baseline['uart_errors']


def ride_demo(device):
    wake_if_dimmed(device)
    state = device.command('STATE')
    for _ in range(3):
        if state['screen'] == 'home':
            break
        device.command('BUTTON 0 1')
        state = device.command('STATE')
    assert state['screen'] == 'home'
    device.tap(80, 260)
    state = device.expect({'screen': 'ride'})
    if state['ride_phase'] == 'running':
        state = device.command('BUTTON 2 1')
    if state['ride_phase'] == 'paused':
        device.tap(80, 260)
    ready = device.expect({'ride_phase': 'ready', 'ride_elapsed_ms': 0,
                           'ride_distance_mm': 0, 'ride_speed_mm_s': 0})

    device.command('RECORD 8000 5')
    started = device.command('BUTTON 2 1')
    assert started['ride_phase'] == 'running' and started['ride_speed_mm_s'] == 5000
    device.wait(2.1)
    running = device.command('STATE')
    assert running['ride_elapsed_ms'] >= 2000
    assert running['ride_distance_mm'] == running['ride_elapsed_ms'] * 5
    device.command('BUTTON 1 1')
    device.expect({'ride_page': ready['ride_page'] ^ 1})
    device.tap(80, 220)
    device.expect({'ride_layout': ready['ride_layout'] ^ 1})
    paused = device.command('BUTTON 2 1')
    assert paused['ride_phase'] == 'paused' and paused['ride_speed_mm_s'] == 0
    device.wait(1.1)
    frozen = device.command('STATE')
    assert frozen['ride_elapsed_ms'] == paused['ride_elapsed_ms']
    assert frozen['ride_distance_mm'] == paused['ride_distance_mm']
    device.command('BUTTON 2 1')
    device.wait(1.1)
    resumed = device.command('STATE')
    assert resumed['ride_phase'] == 'running'
    assert resumed['ride_elapsed_ms'] > frozen['ride_elapsed_ms']
    device.command('BUTTON 2 1')
    device.tap(80, 260)
    device.expect({'ride_phase': 'ready', 'ride_elapsed_ms': 0, 'ride_distance_mm': 0})
    device.command('BUTTON 1 1')
    device.tap(80, 220)
    device.expect({'ride_page': ready['ride_page'], 'ride_layout': ready['ride_layout']})
    device.command('STOP')
    device.capture()


def smoke(device):
    wake_if_dimmed(device)
    if device.command('STATE')['wifi'] != 0:
        device.expect({'wifi': 4}, 45)
        device.wait(8)
        device.expect({'wifi': 4}, 45)
    wake_if_dimmed(device)
    screen = device.command('STATE')['screen']
    if screen != 'controls':
        for _ in range(2):
            if screen == 'home':
                break
            device.command('BUTTON 0 1')
            screen = device.command('STATE')['screen']
        if screen != 'home':
            raise AssertionError(f'Cannot return home from {screen}')
        device.expect({'screen': 'home'})
        device.tap(80, 210)
        device.expect({'screen': 'controls', 'pressed': -1})
    baseline = device.command('STATE')
    device.command('RECORD 15000 5')
    touched = device.command('TOUCH 80 120')
    device.pixel(80, 120, 0x07ff, touched['ms'] + 1)
    device.command('RELEASE')
    device.expect({'x': -1, 'y': -1, 'brightness': baseline['brightness']})
    device.drag([24, 260], [216, 260])
    bright = device.expect({'brightness': 100, 'x': -1})
    device.pixel(216, 260, 0xffff, bright['ms'] + 1)
    device.drag([216, 260], [24, 260])
    dim = device.expect({'brightness': 5})
    device.pixel(24, 260, 0xffff, dim['ms'] + 1)
    for button in [1, 2, 0]:
        device.command(f'BUTTON {button} 1')
    device.tap(80, 210)
    device.expect({
        'screen': 'controls',
        'brightness': 10,
        'buttons': [n + 1 for n in baseline['buttons']],
    })
    device.command('BATTERY 8 3300 1')
    device.expect({'battery': 8, 'power': 1, 'fake_battery': True})
    device.wait(.5)
    device.command('BATTERY 75 4100 0')
    device.expect({'battery': 75, 'power': 0})
    device.wait(.5)
    device.command('LIVE')
    device.expect({'fake_battery': False})
    device.command('STOP')
    if baseline['wifi'] != 0:
        device.expect({'wifi': 4}, 45)
        device.command('WIFI')
        device.expect({'wifi': 5}, 5)
        device.expect({'wifi': 4}, 45)
    device.capture()
    final = device.command('STATE')
    assert final['frame'] > baseline['frame']
    assert final['valid'] > baseline['valid'], 'Companion reception stopped'
    assert final['bad_crc'] == baseline['bad_crc'], 'Companion CRC errors increased'
    assert final['uart_errors'] == baseline['uart_errors'], 'UART errors increased'
    assert final['heap_free'] >= baseline['heap_free'] - 4096, 'Unexpected retained heap growth'
    assert len({f['checksum'] for f in device.frames}) >= 5, 'Recording missed UI changes'


def soak(device, seconds):
    if not 1 <= seconds <= 86400:
        raise ValueError('Soak duration must be 1..86400 seconds')
    baseline = previous = device.command('STATE')
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        device.wait(min(1, deadline - time.monotonic()))
        current = device.command('STATE')
        if current['frame'] <= previous['frame']:
            raise AssertionError('Display stopped advancing or device restarted')
        if current['bad_crc'] != baseline['bad_crc'] or current['uart_errors'] != baseline['uart_errors']:
            raise AssertionError('Input receiver errors increased')
        if current['heap_free'] < baseline['heap_free'] - 4096:
            raise AssertionError('Heap declined by more than 4 KiB')
        previous = current
    device.capture()


def lease_test(device):
    baseline = device.command('STATE')
    device.command('TOUCH 80 100')
    device.command('BATTERY 8 3300 1')
    device.command('RECORD 10000 5')
    device.active = False  # Simulate a host that no longer renews the lease.
    try:
        device.wait(4)
    except RuntimeError as error:
        if str(error) != 'Device test lease expired':
            raise
    else:
        raise AssertionError('Test lease did not expire')
    state = device.command('STATE')
    assert not state['active'] and not state['fake_battery'] and not state['recording']
    assert state['x'] == -1 and state['brightness'] == baseline['brightness']
    assert state['screen'] == baseline['screen'] and state['pressed'] == -1
    try:
        device.command('TOUCH 100 100')
    except RuntimeError as error:
        assert 'NO_SESSION' in str(error)
    else:
        raise AssertionError('Injection was accepted outside a test session')
    device.command('BEGIN')
    device.active = True


def drain_expected_reboot(port, path, timeout=15):
    """Drain one explicitly requested reboot without weakening normal session checks."""
    lock = (ROOT / '.local/usb.lock').open('w')
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    deadline = time.monotonic() + timeout
    quiet_since = None
    seen_boot = False
    recent = bytearray()
    fd = None
    try:
        with path.open('wb') as log:
            while time.monotonic() < deadline:
                if fd is None:
                    try:
                        fd = os.open(port, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
                        tty.setraw(fd)
                        attributes = termios.tcgetattr(fd)
                        attributes[2] = (attributes[2] | termios.CLOCAL | termios.CREAD) & ~termios.HUPCL
                        attributes[4] = attributes[5] = termios.B115200
                        termios.tcsetattr(fd, termios.TCSANOW, attributes)
                    except OSError:
                        time.sleep(.1)
                        continue
                if select.select([fd], [], [], .1)[0]:
                    try:
                        data = os.read(fd, 65536)
                    except OSError:
                        os.close(fd)
                        fd = None
                        continue
                    log.write(data)
                    log.flush()
                    recent.extend(data)
                    seen_boot |= b'CYCLING_BOOT ' in recent
                    if len(recent) > 64:
                        del recent[:-64]
                    quiet_since = time.monotonic()
                elif seen_boot and quiet_since is not None and time.monotonic() - quiet_since >= .5:
                    return
        raise TimeoutError('expected reboot did not produce a boot log')
    finally:
        if fd is not None:
            os.close(fd)
        lock.close()


def crash_test(port, directory):
    def arm(command, folder):
        device = Device(port, folder)
        try:
            device.__enter__()
            before = device.command('STATE')
            armed = device.command(command, expected='ARMED')
            if armed['active']:
                raise AssertionError(f'{command} did not restore the temporary session')
            device.active = False
            return before
        finally:
            device.close()

    before = arm('PANIC', directory / 'panic-arm')
    drain_expected_reboot(port, directory / 'panic-boot.log')
    with Device(port, directory / 'panic-recovered') as device:
        recovered = device.command('STATE')
        if recovered['reset_reason'] != 'software' or recovered['crash_marker'] != 'controlled':
            raise AssertionError('controlled panic marker or reset reason missing')
        if not recovered['crash_firmware']:
            raise AssertionError('retained panic firmware version missing')
        for key in ['brightness', 'dim_timeout', 'dim_brightness', 'timezone']:
            if recovered[key] != before[key]:
                raise AssertionError(f'panic changed preference {key}')
        frame, valid = recovered['frame'], recovered['valid']
        device.wait(2)
        resumed = device.command('STATE')
        if resumed['frame'] <= frame or resumed['valid'] <= valid:
            raise AssertionError('display or companion did not resume after panic')

    arm('RESTART', directory / 'restart-arm')
    drain_expected_reboot(port, directory / 'restart-boot.log')
    with Device(port, directory / 'restart-recovered') as device:
        clean = device.command('STATE')
        if (clean['reset_reason'] != 'software' or clean['crash_marker'] != 'none'
                or clean['crash_firmware']):
            raise AssertionError('consumed panic marker replayed after clean restart')
        for key in ['brightness', 'dim_timeout', 'dim_brightness', 'timezone']:
            if clean[key] != before[key]:
                raise AssertionError(f'clean restart changed preference {key}')
    summary = {
        'before': before,
        'panic_recovered': recovered,
        'resumed': resumed,
        'clean_restart': clean,
    }
    (directory / 'crash-summary.json').write_text(json.dumps(summary, indent=2) + '\n')


def finish_created_ride(device):
    """Best-effort bounded cleanup only after this test acknowledged START."""
    state = device.command('STATE', timeout=3)
    if state['ride_recording'] == 'recording':
        state = device.command('RIDE PAUSE', timeout=8)
    if state['ride_recording'] == 'paused':
        device.command('RIDE FINISH', timeout=8)


def ride_recording_test(port, directory):
    """Exercise explicit initialization, durable demo recovery, and a live ride."""
    def home(device):
        wake_if_dimmed(device)
        state = device.command('STATE')
        for _ in range(3):
            if state['screen'] == 'home':
                return state
            state = device.command('BUTTON 0 1')
        raise AssertionError(f'cannot return home from {state["screen"]}')

    first = Device(port, directory / 'before-restart')
    demo_created = False
    try:
        first.__enter__()
        state = first.command('STATE')
        if state['ride_recording'] == 'scanning':
            deadline = time.monotonic() + 20
            while state['ride_recording'] == 'scanning' and time.monotonic() < deadline:
                first.wait(.25)
                state = first.command('STATE')
        if state['ride_recording'] == 'needs_init':
            state = first.command('RIDE INIT', timeout=60)
            first.command('BEGIN')
        if state['ride_recording'] not in ('ready', 'saved', 'recovered'):
            raise AssertionError(f'ride storage unavailable: {state["ride_recording"]}')

        # An ordinary injected UI ride is restored and never consumes a flash slot.
        baseline_slot = state['recording_slot']
        home(first)
        first.tap(80, 260)
        temporary = first.command('BUTTON 2 1')
        assert temporary['ride_phase'] == 'running'
        assert temporary['recording_slot'] == baseline_slot
        restored = first.command('END')
        assert restored['ride_phase'] == 'ready' and restored['recording_slot'] == baseline_slot

        started = first.command('RIDE START DEMO', timeout=15)
        demo_created = True
        assert started['ride_source'] == 'demo' and started['ride_recording'] == 'recording'
        first.wait(4.4)
        paused = first.command('RIDE PAUSE', timeout=15)
        assert paused['ride_recording'] == 'paused' and paused['recording_active_ms'] >= 4000
        first.command('RIDE RESUME', timeout=15)
        first.wait(2.2)
        before = first.command('STATE')
        armed = first.command('RESTART', expected='ARMED')
        assert not armed['active']
        first.active = False
    except BaseException:
        if demo_created:
            try:
                finish_created_ride(first)
            except BaseException as cleanup_error:
                (directory / 'RECOVERY_REQUIRED.txt').write_text(
                    f'Test-created demo ride may remain open: {cleanup_error}\n'
                    'Restart the device so boot recovery can finalize it.\n')
        raise
    finally:
        first.close()

    drain_expected_reboot(port, directory / 'restart-boot.log')
    with Device(port, directory / 'after-restart') as device:
        live_created = False
        deadline = time.monotonic() + 20
        recovered = device.command('STATE')
        while (recovered['ride_recording'] == 'scanning'
               or recovered['recorded_rides'] <= before['recorded_rides']):
            if time.monotonic() >= deadline:
                raise TimeoutError('interrupted ride was not finalized after scan')
            device.wait(.25)
            recovered = device.command('STATE')
        assert recovered['ride_recording'] == 'recovered'
        assert recovered['recording_slot'] > before['recording_slot']

        try:
            live = device.command('RIDE START LIVE', timeout=15)
            live_created = True
            assert live['ride_source'] == 'live' and live['ride_speed_mm_s'] == -1
            assert live['ride_distance_mm'] == -1
            device.command('BEGIN')
            home(device)
            device.tap(80, 260)
            device.expect({'screen': 'ride', 'ride_source': 'live'})
            device.wait(4.2)
            device.capture()
            device.command('END')
            paused_live = device.command('RIDE PAUSE', timeout=15)
            assert paused_live['recording_active_ms'] >= 4000
            finished = device.command('RIDE FINISH', timeout=15)
            live_created = False
            assert finished['ride_recording'] == 'saved'
            assert finished['recorded_rides'] == recovered['recorded_rides'] + 1
        finally:
            if live_created:
                try:
                    finish_created_ride(device)
                except BaseException as cleanup_error:
                    (directory / 'RECOVERY_REQUIRED.txt').write_text(
                        f'Test-created live ride may remain open: {cleanup_error}\n'
                        'Restart the device so boot recovery can finalize it.\n')

    summary = {
        'temporary': temporary,
        'before_restart': before,
        'recovered': recovered,
        'live_started': live,
        'live_paused': paused_live,
        'finished': finished,
    }
    (directory / 'ride-recording-summary.json').write_text(json.dumps(summary, indent=2) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', default=os.environ.get('CYCLING_PORT', '/dev/ttyACM0'))
    parser.add_argument('--output', type=Path)
    commands = parser.add_subparsers(dest='action', required=True)
    commands.add_parser('state')
    commands.add_parser('capture')
    commands.add_parser('lease-test')
    commands.add_parser('smoke')
    commands.add_parser('wifi-recovery')
    commands.add_parser('ride-demo')
    commands.add_parser('crash-test')
    commands.add_parser('ride-recording-test')
    soak_parser = commands.add_parser('soak')
    soak_parser.add_argument('--seconds', type=float, default=60)
    record = commands.add_parser('record')
    record.add_argument('--seconds', type=float, default=10)
    record.add_argument('--fps', type=int, default=5)
    run = commands.add_parser('run')
    run.add_argument('scenario', type=Path)
    command = commands.add_parser('command')
    command.add_argument('value')
    args = parser.parse_args()
    directory = args.output or ROOT / '.local/tests' / str(time.time_ns())
    if args.action == 'crash-test':
        directory.mkdir(parents=True, exist_ok=False, mode=0o700)
        try:
            crash_test(args.port, directory)
        finally:
            print(f'Evidence: {directory}')
        return
    if args.action == 'ride-recording-test':
        directory.mkdir(parents=True, exist_ok=False, mode=0o700)
        try:
            ride_recording_test(args.port, directory)
        finally:
            print(f'Evidence: {directory}')
        return
    device = Device(args.port, directory)
    try:
        with device:
            if args.action == 'state':
                print(json.dumps(device.command('STATE'), indent=2))
            elif args.action == 'capture':
                device.capture()
            elif args.action == 'lease-test':
                lease_test(device)
            elif args.action == 'soak':
                soak(device, args.seconds)
            elif args.action == 'smoke':
                smoke(device)
            elif args.action == 'wifi-recovery':
                wifi_recovery(device)
            elif args.action == 'ride-demo':
                ride_demo(device)
            elif args.action == 'record':
                device.command(f'RECORD {round(args.seconds * 1000)} {args.fps}')
                device.wait(args.seconds + .5)
                if device.recording.stream not in device.recording.stopped:
                    raise RuntimeError('Recording did not end cleanly')
            elif args.action == 'run':
                scenario(device, json.loads(args.scenario.read_text()))
            elif args.action == 'command':
                print(json.dumps(device.command(args.value), indent=2))
    finally:
        if (directory / 'report.json').exists():
            export(directory)
        print(f'Evidence: {directory}')


if __name__ == '__main__':
    try:
        main()
    except (OSError, RuntimeError, ValueError, AssertionError) as error:
        raise SystemExit(str(error)) from None
