"""Non-interactive C606/virtual scenarios. Results and raw evidence stay in .local/."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import signal
import struct
import subprocess
import sys
import time
import zlib

from harness_av import Capture, discover
from harness_transport import ManualAction, Real, Virtual, usb_node
from usb import ROOT

class Unsupported(RuntimeError):
    pass


VERSION = 1
RECIPES = {
    'input-screen': {'steps': [
        {'op': 'command', 'command': 'INPUT'},
        {'op': 'input', 'gesture': 'tap', 'x': 30, 'y': 30},
        {'op': 'input', 'gesture': 'hold', 'x': 40, 'y': 40, 'duration_ms': 500},
        {'op': 'input', 'gesture': 'swipe', 'x': 20, 'y': 20, 'to_x': 100, 'to_y': 120},
        {'op': 'input', 'gesture': 'button', 'button': 0, 'repeat': 2},
        {'op': 'capture', 'count': 1}, {'op': 'command', 'command': 'INPUT'}]},
    'frames': {'steps': [{'op': 'capture', 'count': 4, 'interval_ms': 250}]},
    'recovery': {'steps': [{'op': 'command', 'command': 'POSITION'},
        {'op': 'recover', 'mode': 'terminal', 'seconds': 2},
        {'op': 'command', 'command': 'POSITION'}, {'op': 'recover', 'mode': 'restart'},
        {'op': 'command', 'command': 'INPUT'}]},
    'shared': {'steps': [{'op': 'command', 'command': 'INFO'},
        {'op': 'command', 'command': 'SETTINGS'}, {'op': 'input', 'gesture': 'tap', 'x': 30, 'y': 30},
        {'op': 'capture', 'count': 2, 'interval_ms': 250}, {'op': 'recover', 'mode': 'restart'},
        {'op': 'command', 'command': 'SETTINGS'}]},
    'av': {'captures': [{'kind': 'camera', 'seconds': 5}, {'kind': 'microphone', 'seconds': 5, 'fixture': True, 'floor_db': -60, 'margin_db': 8}],
        'steps': [{'op': 'capture', 'count': 2, 'interval_ms': 500}]},
}
RECIPES['virtual-foundation'] = {'steps': [
    {'op': 'virtual-command', 'command': 'BRIGHTNESS 61'},
    {'op': 'virtual-command', 'command': 'SAVE'},
    {'op': 'recover', 'mode': 'restart'},
    {'op': 'command', 'command': 'SETTINGS', 'expect': {'brightness': '61'}},
    {'op': 'fixture', 'command': 'STORAGE_FAIL 1 12'},
    {'op': 'virtual-command', 'command': 'BRIGHTNESS 42'},
    {'op': 'virtual-command', 'command': 'SAVE', 'status': 'FAILED'},
    {'op': 'recover', 'mode': 'restart'},
    {'op': 'command', 'command': 'SETTINGS', 'expect': {'brightness': '61'}},
    {'op': 'fixture', 'command': 'POSITION NMEA $GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A'},
    {'op': 'command', 'command': 'POSITION', 'expect': {'transport': 'receiving'}},
    {'op': 'delay', 'seconds': 4},
    {'op': 'command', 'command': 'POSITION', 'expect': {'transport': 'silent'}},
    {'op': 'fixture', 'command': 'POSITION LOSS'},
    {'op': 'command', 'command': 'POSITION', 'expect': {'transport': 'failed'}},
    {'op': 'fixture', 'command': 'BLE CONNECT'},
    {'op': 'fixture', 'command': 'BLE LOSS'},
    {'op': 'fixture', 'command': 'BLE DISCONNECT'},
]}
RECIPES['virtual-sdk'] = {'steps': [
    {'op': 'delay', 'seconds': 3},
    {'op': 'virtual-command', 'command': 'RIDE STATUS', 'expect': {'state': 'ready'}},
    {'op': 'virtual-command', 'command': 'RIDE START', 'status': 'ACCEPTED'},
    {'op': 'delay', 'seconds': 1},
    {'op': 'fixture', 'command': 'BLE CONNECT'},
    {'op': 'fixture', 'command': 'BLE HEART 88'},
    {'op': 'virtual-command', 'command': 'RIDE SENSORS', 'expect': {'heart': 'Some(88)'}},
    {'op': 'delay', 'seconds': 1.2},
    {'op': 'recover', 'mode': 'restart'},
    {'op': 'delay', 'seconds': 5},
    {'op': 'virtual-command', 'command': 'RIDE STATUS', 'expect': {'rides': '1'}},
    {'op': 'virtual-command', 'command': 'EXPORT INFO'},
]}
RECIPES['acceptance'] = {'captures': [
    {'kind': 'camera', 'seconds': 8, 'samples_s': [.5, 1.5, 2.5, 3.5, 7.5]},
    {'kind': 'microphone', 'seconds': 5, 'fixture': True, 'floor_db': -60, 'margin_db': 8,
     'position': 'Laptop microphone and laptop speaker fixture; document unchanged physical placement in evidence.'}],
    'steps': [
        {'op': 'command', 'command': 'BRIGHTNESS 100'},
        {'op': 'command', 'command': 'DISPLAY f800'}, {'op': 'delay', 'seconds': 1},
        {'op': 'command', 'command': 'DISPLAY 07e0'}, {'op': 'delay', 'seconds': 1},
        {'op': 'command', 'command': 'BRIGHTNESS 20'}, {'op': 'delay', 'seconds': 1},
        {'op': 'command', 'command': 'BRIGHTNESS 100'},
        {'op': 'command', 'command': 'POSITION', 'baseline': 'gnss'},
        {'op': 'command', 'command': 'INPUT', 'baseline': 'companion'},
        {'op': 'command', 'command': 'WIFI'}, {'op': 'command', 'command': 'BLE'},
        {'op': 'input', 'gesture': 'tap', 'x': 30, 'y': 30},
        {'op': 'input', 'gesture': 'hold', 'x': 40, 'y': 40, 'duration_ms': 500},
        {'op': 'input', 'gesture': 'drag', 'x': 20, 'y': 20, 'to_x': 100, 'to_y': 120},
        {'op': 'input', 'gesture': 'button', 'button': 0, 'repeat': 2},
        {'op': 'capture', 'count': 1}, {'op': 'capture', 'count': 4, 'interval_ms': 250},
        {'op': 'command', 'command': 'POSITION', 'progress_from': 'gnss', 'counters': ['bytes', 'valid'],
         'unchanged': ['checksum_errors', 'parse_errors', 'dma_losses', 'line_overflows', 'uart_errors']},
        {'op': 'command', 'command': 'INPUT', 'progress_from': 'companion', 'counters': ['companion_valid'],
         'unchanged': ['bad_crc', 'uart_errors', 'touch_errors', 'input_lost']},
        {'op': 'recover', 'mode': 'terminal', 'seconds': 1},
        {'op': 'recover', 'mode': 'usb-reset', 'seconds': 1},
        {'op': 'recover', 'mode': 'restart'},
]}
READ_ONLY = {'HELP', 'INFO', 'STATUS', 'POSITION', 'INPUT', 'BATTERY', 'TIME', 'SETTINGS',
             'ACTIVITY', 'WIFI', 'BLE', 'STORAGE', 'ANT'}


def fields(text):
    return dict(re.findall(r'(\w+)=([^\s]*)', text))


def private(path):
    path = Path(path).resolve()
    if not path.is_relative_to((ROOT / '.local').resolve()):
        raise ValueError('evidence and run state must be inside ignored .local/')
    return path


def atomic_json(path, value):
    temporary = path.with_suffix('.tmp')
    temporary.write_text(json.dumps(value, indent=2))
    temporary.replace(path)


def decode_chunk(chunk, frame, offset, requested):
    length = int(chunk['length'])
    if chunk['frame'] != str(frame) or chunk['offset'] != str(offset) or not 0 < length <= requested:
        raise ValueError('mixed/out-of-range capture chunk')
    encoded = bytes.fromhex(chunk['hex'])
    if len(encoded) > 256:
        raise ValueError('encoded capture chunk exceeds bound')
    if chunk.get('encoding', 'raw') == 'raw':
        payload = encoded
    elif chunk['encoding'] == 'rle565':
        if len(encoded) % 4:
            raise ValueError('truncated RLE tuple')
        payload = bytearray()
        for index in range(0, len(encoded), 4):
            count, pixel = struct.unpack_from('<HH', encoded, index)
            if count == 0 or len(payload) + count * 2 > length:
                raise ValueError('invalid RLE run length')
            payload.extend(struct.pack('<H', pixel) * count)
    else:
        raise ValueError('unsupported capture encoding')
    if len(payload) != length or zlib.crc32(payload) != int(chunk['crc32'], 16):
        raise ValueError('corrupt capture chunk')
    return payload


def png_rgb565(path, payload, width, height):
    if len(payload) != width * height * 2:
        raise ValueError('pixel byte count does not match geometry')
    raw = bytearray()
    for y in range(height):
        raw.append(0)
        for x in range(width):
            value = struct.unpack_from('<H', payload, (y * width + x) * 2)[0]
            raw.extend(((value >> 11) * 255 // 31, ((value >> 5) & 63) * 255 // 63, (value & 31) * 255 // 31))
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
    path.write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 2, 0, 0, 0))
                     + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b''))


def events_for(step):
    if 'events' in step:
        events = step['events']
    else:
        gesture = step.get('gesture', 'tap')
        duration = int(step.get('duration_ms', 300 if gesture in ('swipe', 'drag') else 100))
        x, y = int(step.get('x', 0)), int(step.get('y', 0))
        if gesture == 'button':
            events = [{'at_ms': index * duration, 'event': f'BUTTON {int(step.get("button", 0))}'}
                      for index in range(int(step.get('repeat', 1)))]
        elif gesture in ('tap', 'hold'):
            events = [{'at_ms': 0, 'event': f'DOWN {x} {y}'}, {'at_ms': duration, 'event': 'UP'}]
        elif gesture in ('swipe', 'drag'):
            tx, ty = int(step['to_x']), int(step['to_y'])
            events = [{'at_ms': 0, 'event': f'DOWN {x} {y}'}]
            events += [{'at_ms': duration * i // 8, 'event': f'MOVE {x + (tx-x)*i//8} {y + (ty-y)*i//8}'} for i in range(1, 9)]
            events.append({'at_ms': duration + 10, 'event': 'UP'})
        else:
            raise ValueError('unknown gesture; button holds are unsupported by physical event evidence')
    if not 1 <= len(events) <= 64:
        raise ValueError('input requires 1..64 events')
    previous, held = -1, False
    for event in events:
        at = event['at_ms']
        if type(at) is not int or not previous <= at <= 30000 or at < 0:
            raise ValueError('event offsets must be ordered integers in 0..30000 ms')
        previous = at
        words = event['event'].split()
        if not words or words[0] not in ('BUTTON', 'DOWN', 'MOVE', 'UP', 'CANCEL'):
            raise ValueError('unsupported input event')
        sizes = {'BUTTON': 2, 'DOWN': 3, 'MOVE': 3, 'UP': 1, 'CANCEL': 1}
        if len(words) != sizes[words[0]] or any(not w.isdecimal() for w in words[1:]):
            raise ValueError('invalid event arguments')
        if words[0] == 'DOWN':
            if held:
                raise ValueError('duplicate touch down')
            held = True
        elif words[0] in ('MOVE', 'UP'):
            if not held:
                raise ValueError('touch move/up without down')
            if words[0] == 'UP':
                held = False
        elif words[0] == 'CANCEL':
            held = False
    if held:
        raise ValueError('sequence must end with touch up/cancel')
    return events


def preflight(scenario):
    steps = scenario.get('steps')
    if not isinstance(steps, list) or not 1 <= len(steps) <= 256:
        raise ValueError('scenario requires 1..256 steps')
    baselines = {}
    for step in steps:
        op = step.get('op')
        if 'progress_from' in step and baselines.get(step['progress_from']) != step.get('command'):
            raise ValueError('progress assertion requires an earlier baseline for the same command')
        if 'baseline' in step:
            baselines[step['baseline']] = step.get('command')
        if op in ('command', 'wait'):
            command = step['command']
            words = command.split()
            permitted = len(words) == 1 and words[0] in READ_ONLY
            permitted |= bool(re.fullmatch(r'DISPLAY [0-9a-fA-F]{1,4}', command))
            permitted |= bool(re.fullmatch(r'BRIGHTNESS (?:100|[0-9]{1,2})', command))
            if not permitted or (op == 'wait' and len(words) != 1):
                raise ValueError('scenario command is not a supported read or temporary display/brightness operation; persistence mutations require dedicated protected workflows')
            if op == 'wait' and not 0 < float(step.get('timeout_s', 10)) <= 120:
                raise ValueError('wait timeout must be 0..120 seconds')
        elif op in ('fixture', 'virtual-command'):
            text = step['command']
            if not isinstance(text, str) or not text or len(text) > 100 or any(c in text for c in '\n\r\x00'):
                raise ValueError('invalid virtual command framing')
            if op == 'fixture' and text.split()[0] not in ('ADVANCE', 'POSITION', 'BLE', 'STORAGE_FAIL', 'DISPLAY_FAIL'):
                raise ValueError('unknown virtual fixture')
            if op == 'virtual-command' and text.split()[0] not in READ_ONLY | {'RIDE', 'EXPORT', 'RADAR', 'SAVE', 'BRIGHTNESS', 'TIMEZONE', 'IDLE'}:
                raise ValueError('unsupported virtual test-media command')
        elif op == 'input':
            events_for(step)
        elif op == 'capture':
            if not 1 <= int(step.get('count', 1)) <= 4 or not 100 <= int(step.get('interval_ms', 250)) <= 10000:
                raise ValueError('capture requires 1..4 frames at 100..10000 ms intervals')
        elif op == 'recover':
            if step.get('mode', 'terminal') not in ('terminal', 'restart', 'usb-reset'):
                raise ValueError('host detach and VBUS removal are unsupported; recovery modes are terminal/restart/usb-reset')
            if not 0 <= float(step.get('seconds', 1)) <= 60:
                raise ValueError('missing-host interval must be 0..60 seconds')
        elif op == 'delay':
            if not 0 <= float(step['seconds']) <= 60:
                raise ValueError('delay must be 0..60 seconds')
        elif op == 'manual':
            if not step.get('action'):
                raise ValueError('manual step requires an exact action')
        else:
            raise ValueError('unknown scenario operation')
    if len(scenario.get('captures', [])) > 2:
        raise ValueError('at most one camera and one microphone capture per run')
    kinds = [capture['kind'] for capture in scenario.get('captures', [])]
    if len(set(kinds)) != len(kinds) or any(kind not in ('camera', 'microphone') for kind in kinds):
        raise ValueError('capture kinds must be unique camera/microphone')


class Runner:
    def __init__(self, transport, output):
        self.transport, self.output = transport, output
        self.session = self.boot = None
        self.commands = self.sequence = 0
        self.trace = (output / 'commands.jsonl').open('w')
        self.caps = {}
        self.last_lease = time.monotonic()
        self.observations = {}

    def command(self, command, statuses=('OK',), timeout=30):
        request = f'CMD {self.transport.request_id + 1} {command}'
        if len(request.encode('ascii')) > 128 or any(c in command for c in '\n\r\x00'):
            raise ValueError('invalid command framing')
        if self.session and not command.startswith('HARNESS ') and time.monotonic() - self.last_lease > 30:
            self.operation('PING')
        started = time.monotonic()
        self.commands += 1
        reply = self.transport.terminal_command(command, timeout=timeout)
        self.trace.write(json.dumps({'host_start_s': started, 'host_end_s': time.monotonic(), 'command': command, 'reply': reply}) + '\n')
        self.trace.flush()
        if reply['status'] not in statuses:
            if reply['status'] in ('UNSUPPORTED', 'UNAVAILABLE'):
                raise Unsupported(reply['status'])
            raise RuntimeError(f'{command.split()[0]} returned {reply["status"]}')
        return fields(reply['data'])

    def operation(self, command, statuses=('OK',)):
        result = self.command(f'HARNESS {self.session} {command}', statuses)
        self.last_lease = time.monotonic()
        if result.get('boot') != self.boot or result.get('session') != ('0' if command == 'CLOSE' else self.session):
            raise RuntimeError('stale session/boot reply; operation outcome uncertain')
        return result

    def open_session(self):
        self.caps = self.command('HARNESS CAPS')
        nonce = secrets.randbelow(1 << 32)
        result = self.command(f'HARNESS OPEN {nonce} 120000')
        if result.get('nonce') != str(nonce) or result.get('boot') != self.caps.get('boot'):
            raise RuntimeError('session open correlation mismatch')
        self.session, self.boot = result['session'], result['boot']

    def delay(self, seconds):
        remaining = seconds
        while remaining > 0:
            span = min(30, remaining)
            self.transport.delay(span)
            remaining -= span
            if self.session:
                self.operation('PING')

    def wait(self, command, condition, timeout=10, harness=False):
        stop = time.monotonic() + timeout
        for _ in range(2401):
            result = self.operation(command) if harness else self.command(command)
            if all(result.get(key) == str(value).lower() if isinstance(value, bool) else result.get(key) == str(value)
                   for key, value in condition.items()):
                return result
            if time.monotonic() >= stop:
                break
            self.delay(.05)
        raise TimeoutError('condition did not complete within bounded wait')

    def inject(self, step):
        events = events_for(step)
        self.sequence += 1
        self.operation(f'INPUT BEGIN {self.sequence}')
        for event in events:
            self.operation(f'INPUT ADD {self.sequence} {event["at_ms"]} {event["event"]}', ('ACCEPTED',))
        self.operation(f'INPUT RUN {self.sequence}', ('ACCEPTED',))
        result = self.wait('INPUT STATUS', {'state': 'completed'}, 35, harness=True)
        if result['accepted'] != result['delivered'] or result['losses'] != '0' or result['held'] != 'false':
            raise RuntimeError('input sequence lost events or left held state')
        return result

    def capture(self, step, index):
        started = time.monotonic()
        count, interval = int(step.get('count', 1)), int(step.get('interval_ms', 250))
        self.operation(f'CAPTURE START {count} {interval}', ('ACCEPTED',))
        status = self.wait('CAPTURE STATUS', {'state': 'completed'}, count * interval / 1000 + 10, harness=True)
        frames = [int(value) for value in status['frames'].split(',') if value]
        if len(frames) != count or len(set(frames)) != count:
            raise RuntimeError('incomplete/mixed capture sequence')
        results = []
        for frame in frames:
            meta = self.operation(f'CAPTURE META {frame}')
            if meta['complete'] != 'true' or meta['submitted'] != 'true' or meta['format'] != 'RGB565LE':
                raise RuntimeError('capture is incomplete or display submission failed')
            size, width, height = int(meta['bytes']), int(meta['width']), int(meta['height'])
            if size != width * height * 2 or not 0 < size <= 1024 * 1024:
                raise ValueError('invalid capture geometry/size')
            mismatch = 'expected_crc32' in step and meta['crc32'] != step['expected_crc32']
            if step.get('checksum_only') and 'expected_crc32' in step and not mismatch:
                results.append(meta)
                continue
            data = bytearray()
            while len(data) < size:
                offset, length = len(data), min(int(self.caps['chunk_bytes']), size - len(data))
                chunk = self.operation(f'CAPTURE READ {frame} {offset} {length}')
                payload = decode_chunk(chunk, frame, offset, length)
                data.extend(payload)
            if zlib.crc32(data) != int(meta['crc32'], 16):
                raise ValueError('capture frame integrity mismatch')
            path = self.output / f'step-{index}-frame-{frame}.png'
            png_rgb565(path, data, width, height)
            results.append({**meta, 'path': str(path), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()})
            if mismatch:
                raise AssertionError(f'captured checksum differs; actual image retained at {path}')
        return {**status, 'frames': results, 'elapsed_s': time.monotonic() - started,
                'transfer_bytes': sum(int(frame['bytes']) for frame in results if 'path' in frame),
                'physical_panel_verified': False}

    def step(self, step, index):
        op = step['op']
        if op == 'command':
            result = self.command(step['command'])
            for key, expected in step.get('expect', {}).items():
                if result.get(key) != str(expected):
                    raise AssertionError(f'command assertion failed for {key}')
            if 'progress_from' in step:
                baseline = self.observations[step['progress_from']]
                delta = {key: int(result[key]) - int(baseline[key]) for key in step.get('counters', []) + step.get('unchanged', [])}
                if any(delta[key] <= 0 for key in step.get('counters', [])) or any(delta[key] != 0 for key in step.get('unchanged', [])):
                    raise AssertionError('acquisition failed progress/loss assertions')
                result['delta'] = delta
            if 'baseline' in step:
                self.observations[step['baseline']] = result
            return result
        if op == 'wait':
            return self.wait(step['command'], step['equals'], float(step.get('timeout_s', 10)))
        if op == 'fixture':
            result = self.transport.fixture(step['command'])
            if result['status'] != step.get('status', 'OK'):
                raise RuntimeError('fixture returned ' + result['status'])
            return result
        if op == 'virtual-command':
            result = self.command(step['command'], (step.get('status', 'OK'),))
            for key, expected in step.get('expect', {}).items():
                if result.get(key) != str(expected):
                    raise AssertionError(f'virtual command assertion failed for {key}')
            return result
        if op == 'input':
            return self.inject(step)
        if op == 'capture':
            return self.capture(step, index)
        if op == 'delay':
            self.delay(float(step['seconds']))
            return {}
        if op == 'manual':
            raise ManualAction(step['action'])
        if op == 'recover':
            previous = self.boot
            if self.session:
                self.operation('CLOSE')
                self.session = None
            result = self.transport.recover(step.get('mode', 'terminal'), float(step.get('seconds', 1)))
            caps = self.command('HARNESS CAPS')
            self.boot = caps.get('boot')
            if caps.get('input') == 'supported':
                self.open_session()
            changed = previous != self.boot
            if changed != (step.get('mode', 'terminal') == 'restart'):
                raise RuntimeError('observed boot identity does not match requested disruption')
            return {**result, 'boot_changed': changed, 'previous_boot': previous, 'boot': self.boot}
        raise ValueError('unknown operation')


def compact(report):
    return {key: report[key] for key in ('version', 'run_id', 'status', 'elapsed_s', 'host_invocations',
            'agent_tool_calls', 'protocol_requests', 'report', 'manual_action', 'error', 'cleanup') if key in report} | {
            'steps': [{key: step[key] for key in ('index', 'op', 'status', 'elapsed_s', 'error') if key in step}
                      for step in report.get('steps', [])]}


def execute(args, scenario, output):
    started = time.monotonic()
    report = {'version': VERSION, 'run_id': args.run_id or output.name, 'status': 'running', 'steps': [],
              'host_invocations': 1, 'agent_tool_calls': args.agent_tool_calls,
              'report': str(output / 'report.json'), 'host_started_unix_s': time.time(),
              'clock_correlation': 'Device reply ms is sampled at loop start and may predate host send by loop work. Host send/receive times are retained; exact device/AV alignment is unmeasured, including AV startup/buffering.',
              'limits': ['Synthetic input does not verify physical sensing.', 'Submitted pixels do not verify the panel.',
                         'USB recovery does not remove VBUS.', 'Virtual runs do not establish hardware timing.']}
    atomic_json(output / 'report.json', report)
    runner = transport = None
    captures, active_captures = [], []
    original = None
    try:
        preflight(scenario)
        if args.backend != 'virtual' and any(step['op'] in ('fixture', 'virtual-command') for step in scenario['steps']):
            raise ValueError('fixture and test-media commands are virtual-only; real data is preserved')
        if any(step['op'] == 'recover' and step.get('mode') == 'usb-reset' for step in scenario['steps']):
            if args.backend == 'virtual':
                raise Unsupported('UNSUPPORTED')
            node = usb_node(args.port)
            path = f'/dev/bus/usb/{int((node / "busnum").read_text()):03}/{int((node / "devnum").read_text()):03}'
            if not os.access(path, os.W_OK):
                raise ValueError('USB bus reset requires write access to the selected device node')
        for config in scenario.get('captures', []):
            capture = Capture(config['kind'], config, output / config['kind'])
            capture.preflight()
            captures.append(capture)
        transport = Virtual(output, args.state_dir, args.sdk, not args.no_harness) if args.backend == 'virtual' else Real(args.port, output / 'usb.log')
        transport.__enter__()
        runner = Runner(transport, output)
        report['build'] = runner.command('INFO')
        if not transport.virtual and any(step['op'] == 'recover' and step.get('mode') in ('restart', 'usb-reset') for step in scenario['steps']):
            if report['build'].get('cycling') != 'false' or report['build'].get('recording') != 'false':
                raise ValueError('real restart recipes require base firmware with no recording consumer; preserve active SDK work')
        original = runner.command('SETTINGS')
        report['initial_settings'] = original
        report['initial_status'] = runner.command('STATUS')
        report['capabilities'] = caps = runner.command('HARNESS CAPS')
        runner.boot = caps.get('boot')
        needed = {step['op'] for step in scenario['steps']} & {'input', 'capture'}
        for capability in needed:
            if caps.get(capability) != 'supported':
                report['status'] = 'unsupported'
                raise ValueError(f'{capability} is {caps.get(capability, "unsupported")} in this build/device')
        for step in scenario['steps']:
            if step['op'] in ('command', 'wait', 'virtual-command') and caps.get('ordinary'):
                if step['command'].split()[0] not in caps['ordinary'].split(','):
                    report['status'] = 'unsupported'
                    raise ValueError('scenario uses a command unsupported by this backend')
            if step['op'] == 'input':
                for event in events_for(step):
                    words = event['event'].split()
                    if words[0] in ('DOWN', 'MOVE') and (int(words[1]) >= int(caps['width']) or int(words[2]) >= int(caps['height']) or caps['touch'] != 'supported'):
                        raise ValueError('input coordinate/touch capability unavailable')
                    if words[0] == 'BUTTON' and words[1] not in caps['buttons'].split(','):
                        raise ValueError('button is not advertised by device')
        if needed or caps.get('input') == 'supported':
            runner.open_session()
        report['boot'], report['session'] = runner.boot, runner.session
        for capture in captures:
            active_captures.append(capture)
            capture.start()
        for index, step in enumerate(scenario['steps']):
            entry = {'index': index, 'op': step['op'], 'status': 'running', 'host_started_s': time.monotonic()}
            report['steps'].append(entry)
            atomic_json(output / 'report.json', report)
            try:
                entry['result'] = runner.step(step, index)
                entry['status'] = 'pass'
            except BaseException as error:
                entry['status'] = 'needs-manual-action' if isinstance(error, ManualAction) else 'unsupported' if isinstance(error, Unsupported) else 'fail'
                entry['error'] = str(error)
                raise
            finally:
                entry['elapsed_s'] = time.monotonic() - entry['host_started_s']
        report['final_status'] = runner.command('STATUS')
        report['status'] = 'pass'
    except Unsupported as error:
        report.update(status='unsupported' if str(error) == 'UNSUPPORTED' else 'inconclusive', error=str(error))
    except ManualAction as error:
        report.update(status='needs-manual-action', manual_action=str(error))
    except (Exception, KeyboardInterrupt) as error:
        if report['status'] == 'running':
            report['status'] = 'fail'
        report['error'] = str(error) or 'interrupted; cleanup attempted'
    finally:
        cleanup = {'input_capture': 'not-started', 'preferences': 'unchanged', 'readers': 'closed'}
        if runner:
            try:
                if runner.session:
                    try:
                        runner.operation('CLOSE')
                        runner.session = None
                        cleanup['input_capture'] = 'closed'
                    except (Exception, KeyboardInterrupt) as error:
                        cleanup['input_capture'] = 'uncertain; device lease expiry cancels held input'
                        cleanup['cancel_error'] = str(error)
                        report['status'] = 'inconclusive'
                if original:
                    actual = runner.command('SETTINGS')
                    if actual.get('brightness') != original.get('brightness'):
                        runner.command(f'BRIGHTNESS {original["brightness"]}')
                        actual = runner.command('SETTINGS')
                    if actual.get('timezone') != original.get('timezone'):
                        runner.command(f'TIMEZONE {original["timezone"]}')
                    if any(actual.get(key) != original.get(key) for key in ('dim_timeout', 'dim_brightness')):
                        runner.command(f'IDLE {original["dim_timeout"]} {original["dim_brightness"]}')
                    actual = runner.command('SETTINGS')
                    for key in ('brightness', 'timezone', 'dim_timeout', 'dim_brightness'):
                        if actual.get(key) != original.get(key):
                            raise RuntimeError(f'preference restoration mismatch: {key}')
                    cleanup['preferences'] = 'verified'
            except (Exception, KeyboardInterrupt) as error:
                cleanup['error'] = str(error)
                cleanup['preferences'] = 'uncertain; inspect before retrying mutation'
                report['status'] = 'inconclusive'
            finally:
                runner.trace.close()
                report['command_requests'] = getattr(transport, 'request_count', runner.commands)
                report['fixture_requests'] = getattr(transport, 'fixture_count', 0)
                report['protocol_requests'] = report['command_requests'] + report['fixture_requests']
        report['captures'] = []
        for capture in active_captures:
            try:
                result = capture.finish(cancel=report['status'] != 'pass')
                report['captures'].append(result)
                if result['status'] != 'pass' and report['status'] == 'pass':
                    report['status'] = result['status']
            except (Exception, KeyboardInterrupt) as error:
                report['captures'].append({'status': 'fail', 'error': str(error)})
                report['status'] = 'fail'
        if transport:
            transport.__exit__(None, None, None)
        for index in range(len(report['steps']), len(scenario.get('steps', []))):
            report['steps'].append({'index': index, 'op': scenario['steps'][index]['op'], 'status': 'skipped'})
        report['cleanup'] = cleanup
        report['artifacts'] = [str(path) for path in output.rglob('*.png')]
        report['elapsed_s'] = time.monotonic() - started
        atomic_json(output / 'report.json', report)
    print(json.dumps(compact(report)), flush=True)
    return 0 if report['status'] == 'pass' else 2 if report['status'] in ('unsupported', 'needs-manual-action', 'inconclusive') else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['run', 'start', 'wait', 'cancel', 'recipes', 'av-discover'])
    parser.add_argument('name', nargs='?', help='recipe name or run ID for wait/cancel')
    parser.add_argument('--scenario', type=Path)
    parser.add_argument('--run-id', help=argparse.SUPPRESS)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--backend', choices=['real', 'virtual'], default='real')
    parser.add_argument('--port', default=os.environ.get('CYCLING_PORT', '/dev/ttyACM0'))
    parser.add_argument('--state-dir', type=Path)
    parser.add_argument('--sdk', action='store_true')
    parser.add_argument('--no-harness', action='store_true')
    parser.add_argument('--seconds', type=float, default=30, help='bounded wait duration, at most 60 seconds')
    parser.add_argument('--agent-tool-calls', type=int, help='operator-recorded actual calls for this invocation; omitted if unknown')
    args = parser.parse_args()
    if args.action == 'recipes':
        print(json.dumps({'version': VERSION, 'recipes': RECIPES}))
        return 0
    if args.action == 'av-discover':
        print(json.dumps(discover()))
        return 0
    if args.action in ('wait', 'cancel'):
        output = private(args.output or ROOT / '.local/harness/runs' / (args.name or ''))
        if args.action == 'cancel':
            pid = int((output / 'pid').read_text())
            command = Path(f'/proc/{pid}/cmdline').read_bytes()
            if str(output).encode() not in command or b'harness.py' not in command:
                raise ValueError('worker identity mismatch; refusing to signal reused PID')
            os.kill(pid, signal.SIGTERM)
        stop = time.monotonic() + min(60, max(0, args.seconds))
        while True:
            try:
                report = json.loads((output / 'report.json').read_text())
            except FileNotFoundError:
                report = {'version': VERSION, 'run_id': output.name, 'status': 'running'}
            if report['status'] != 'running' or time.monotonic() >= stop:
                print(json.dumps(compact(report)))
                return 0 if report['status'] in ('pass', 'running') else 2
            time.sleep(.2)
    scenario = json.loads(args.scenario.read_text()) if args.scenario else RECIPES.get(args.name or 'shared')
    if scenario is None:
        raise ValueError('unknown recipe; use recipes')
    preflight(scenario)
    output = private(args.output or ROOT / '.local/harness/runs' / f'{time.time_ns()}-{secrets.token_hex(3)}')
    if args.state_dir:
        args.state_dir = private(args.state_dir)
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    if args.action == 'start':
        scenario_path = output / 'scenario.json'
        atomic_json(scenario_path, scenario)
        worker_output = output / 'result'
        cmd = [sys.executable, str(Path(__file__).resolve()), 'run', '--scenario', str(scenario_path),
               '--output', str(worker_output), '--run-id', output.name, '--backend', args.backend, '--port', args.port]
        for flag in ('sdk', 'no_harness'):
            if getattr(args, flag):
                cmd.append('--' + flag.replace('_', '-'))
        if args.state_dir:
            cmd += ['--state-dir', str(args.state_dir)]
        if args.agent_tool_calls is not None:
            cmd += ['--agent-tool-calls', str(args.agent_tool_calls)]
        with (output / 'worker.log').open('wb') as log:
            process = subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        ready = time.monotonic() + 3
        while not (worker_output / 'report.json').exists() and process.poll() is None and time.monotonic() < ready:
            time.sleep(.02)
        if not (worker_output / 'report.json').exists():
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=3)
            raise RuntimeError('worker did not initialize; see private worker.log')
        (output / 'pid').write_text(str(process.pid))
        # wait/cancel operates on the stable parent path, report links to worker result.
        (output / 'report.json').symlink_to(worker_output / 'report.json')
        print(json.dumps({'version': VERSION, 'run_id': output.name, 'status': 'running', 'path': str(output),
                          'wait': f'mise run harness -- wait {output.name} --seconds 30'}))
        return 0
    def interrupted(*_):
        raise KeyboardInterrupt()
    signal.signal(signal.SIGTERM, interrupted)
    return execute(args, scenario, output)


if __name__ == '__main__':
    try:
        raise SystemExit(main())
    except (OSError, ValueError, RuntimeError) as error:
        print(json.dumps({'version': VERSION, 'status': 'fail', 'error': str(error)}))
        raise SystemExit(1)
