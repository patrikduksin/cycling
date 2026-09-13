"""Bounded private camera/microphone evidence. No device USB access."""
import array
import cmath
import hashlib
import json
import math
from pathlib import Path
import shutil
import subprocess
import time
import wave


def command(args):
    return subprocess.run(args, check=True, capture_output=True, text=True, timeout=10).stdout


def discover():
    cameras = []
    for path in sorted(Path('/sys/class/video4linux').glob('video*')):
        name = (path / 'name').read_text().strip()
        cameras.append({'path': '/dev/' + path.name, 'name': name})
    sources = []
    if shutil.which('pactl'):
        for source in json.loads(command(['pactl', '-f', 'json', 'list', 'sources'])):
            if source['name'].endswith('.monitor') or source.get('description', '').startswith('Monitor of '):
                continue
            sources.append({key: source.get(key) for key in
                            ('name', 'description', 'mute', 'volume', 'sample_specification', 'properties')})
    return {'cameras': cameras, 'microphones': sources, 'ffmpeg': shutil.which('ffmpeg') is not None}


def fft(values):
    """Radix-2 FFT for a bounded diagnostic spectrum, no numerical dependency."""
    n = len(values)
    if n == 1:
        return values
    even, odd = fft(values[::2]), fft(values[1::2])
    factors = [cmath.exp(-2j * math.pi * k / n) * odd[k] for k in range(n // 2)]
    return [even[k] + factors[k] for k in range(n // 2)] + [even[k] - factors[k] for k in range(n // 2)]


def analyze_audio(path, noise_seconds=.4, margin_db=12, floor_db=-45):
    with wave.open(str(path), 'rb') as stream:
        if stream.getsampwidth() != 2 or stream.getnchannels() != 1:
            raise ValueError('analysis requires mono signed 16-bit PCM')
        rate = stream.getframerate()
        samples = array.array('h', stream.readframes(stream.getnframes()))
    import sys
    if sys.byteorder != 'little':
        samples.byteswap()
    if len(samples) < rate * noise_seconds:
        raise ValueError('recording too short for background measurement')
    block = max(1, rate // 100)
    rms = [math.sqrt(sum((v / 32768) ** 2 for v in samples[i:i + block]) / len(samples[i:i + block]))
           for i in range(0, len(samples), block)]
    noise = sorted(rms[:max(1, int(noise_seconds * rate / block))])
    background = noise[len(noise) // 2]
    threshold = max(10 ** (floor_db / 20), background * 10 ** (margin_db / 20))
    spans, start = [], None
    for i, level in enumerate(rms + [0]):
        if level >= threshold and start is None:
            start = i
        elif level < threshold and start is not None:
            if i - start >= 3:
                spans.append((start * block, min(i * block, len(samples))))
            start = None
    events = []
    for first, last in spans:
        segment = samples[first:last]
        n = 1 << min(13, (len(segment)).bit_length() - 1)
        selected = segment[(len(segment) - n) // 2:(len(segment) + n) // 2]
        spectrum = fft([value * (.5 - .5 * math.cos(2 * math.pi * i / max(1, n - 1)))
                        for i, value in enumerate(selected)])
        peak = max(range(1, n // 2), key=lambda i: abs(spectrum[i]))
        level = math.sqrt(sum((v / 32768) ** 2 for v in segment) / len(segment))
        events.append({'onset_s': first / rate, 'duration_s': (last - first) / rate,
                       'dominant_hz': peak * rate / n, 'frequency_bin_hz': rate / n,
                       'rms_dbfs': 20 * math.log10(max(level, 1e-12))})
    clipped = sum(abs(v) >= 32760 for v in samples)
    return {'duration_s': len(samples) / rate, 'sample_rate': rate, 'events': events,
            'repetitions': len(events), 'background_dbfs': 20 * math.log10(max(background, 1e-12)),
            'threshold_dbfs': 20 * math.log10(threshold), 'clipped_samples': clipped,
            'timing_resolution_s': block / rate, 'noise_reference_s': noise_seconds,
            'status': 'inconclusive' if clipped or not events else 'pass',
            'limits': 'Recorded dBFS only, not SPL. Initial background must be quiet; thresholds do not identify the sound source.'}


def tone_fixture(path, frequency=1000, amplitude=.15):
    rate = 16000
    values = array.array('h')
    for i in range(rate * 3):
        t = i / rate
        active = .7 <= t < 1.1 or 1.6 <= t < 2.0
        values.append(round(32767 * amplitude * math.sin(2 * math.pi * frequency * t)) if active else 0)
    import sys
    if sys.byteorder != 'little':
        values.byteswap()
    with wave.open(str(path), 'wb') as stream:
        stream.setparams((1, 2, rate, 0, 'NONE', 'not compressed'))
        stream.writeframes(values.tobytes())


class Capture:
    """An owned ffmpeg process with an independent duration cap and bounded join."""
    def __init__(self, kind, config, output):
        self.kind, self.config, self.output = kind, config, Path(output)
        self.process = self.log = self.fixture = None
        self.started = None
        self.duration = float(config.get('seconds', 3))
        if not .5 <= self.duration <= 60:
            raise ValueError('AV duration must be 0.5..60 seconds')
        self.path = self.output / ('camera.mkv' if kind == 'camera' else 'microphone.wav')

    def preflight(self):
        inventory = discover()
        if not inventory['ffmpeg']:
            raise ValueError('ffmpeg unavailable')
        if self.kind == 'camera':
            selected = self.config.get('device')
            matches = [item for item in inventory['cameras'] if item['path'] == selected] if selected else [
                item for item in inventory['cameras'] if 'camera' in item['name'].lower()]
            if len(matches) != 1:
                raise ValueError('select one camera path from harness av-discover')
            self.setup = matches[0]
            self.setup['controls'] = command(['v4l2-ctl', '-d', self.setup['path'], '--list-ctrls'])
        else:
            selected = self.config.get('source')
            matches = [item for item in inventory['microphones'] if item['name'] == selected] if selected else [
                item for item in inventory['microphones'] if 'microphone' in item['description'].lower()]
            if not selected and len(matches) > 1:
                matches = [item for item in matches if 'headset' not in item['description'].lower()]
            if len(matches) != 1:
                raise ValueError('select one microphone source from harness av-discover')
            self.setup = matches[0]
            if self.setup['mute']:
                raise ValueError('selected microphone is muted')
            self.setup['position'] = self.config.get('position', 'unrecorded; relative comparisons limited')
            self.setup['processing'] = 'Existing host route retained; AGC/noise processing not verified. Relative levels limited.'
        if self.kind == 'microphone' and self.config.get('fixture'):
            if not shutil.which('paplay'):
                raise ValueError('paplay unavailable for audible fixture')
            self.setup['fixture_sink'] = self.config.get('sink') or command(['pactl', 'get-default-sink']).strip()
            self.setup['fixture'] = 'Laptop playback: two 1 kHz bursts, 0.4 s each, amplitude 0.7 PCM; not C606 sound.'
        samples = self.config.get('samples_s', [])
        if len(samples) > 8 or any(not 0 <= float(value) < self.duration for value in samples):
            raise ValueError('camera preview times must be within capture, at most eight')
        return self.setup

    def start(self):
        self.output.mkdir(parents=True, exist_ok=True, mode=0o700)
        args = ['ffmpeg', '-nostdin', '-hide_banner', '-loglevel', 'warning', '-y']
        if self.kind == 'camera':
            args += ['-f', 'v4l2', '-input_format', self.config.get('format', 'nv12'),
                     '-video_size', self.config.get('size', '1280x720'), '-i', self.setup['path'],
                     '-t', str(self.duration), '-an', '-c:v', 'ffv1', str(self.path)]
        else:
            args += ['-f', 'pulse', '-i', self.setup['name'], '-t', str(self.duration),
                     '-ac', '1', '-ar', '16000', '-c:a', 'pcm_s16le', str(self.path)]
        self.log = (self.output / f'{self.kind}.log').open('wb')
        self.started = time.monotonic()
        self.process = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=self.log)
        if self.kind == 'microphone' and self.config.get('fixture'):
            fixture = self.output / 'fixture.wav'
            tone_fixture(fixture, amplitude=.7)
            self.fixture = subprocess.Popen(['paplay', '--volume=65536', '--device=' + self.setup['fixture_sink'], str(fixture)],
                                            stdout=subprocess.DEVNULL, stderr=self.log)

    def finish(self, cancel=False):
        if self.process is None:
            if self.log is not None:
                self.log.close()
            return {'status': 'skipped'}
        if cancel and self.process.poll() is None:
            self.process.terminate()
        try:
            self.process.wait(timeout=max(1, self.duration + 8 - (time.monotonic() - self.started)))
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=3)
        finally:
            if self.fixture is not None:
                if self.fixture.poll() is None:
                    self.fixture.terminate()
                try:
                    self.fixture.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    self.fixture.kill()
                    self.fixture.wait(timeout=2)
            self.log.close()
        result = {'status': 'fail' if self.process.returncode else 'pass', 'path': str(self.path),
                  'host_started_monotonic_s': self.started, 'host_finished_monotonic_s': time.monotonic(),
                  'configuration': self.setup, 'settings_changed': False,
                  'clock_uncertainty': 'Process launch precedes first sample by unmeasured backend startup/buffering latency.'}
        if self.path.exists():
            result['sha256'] = hashlib.sha256(self.path.read_bytes()).hexdigest()
        if self.process.returncode == 0 and self.kind == 'microphone':
            result['analysis'] = analyze_audio(self.path, margin_db=float(self.config.get('margin_db', 12)), floor_db=float(self.config.get('floor_db', -45)))
            if self.fixture is not None:
                result['fixture_exit'] = self.fixture.returncode
                events = result['analysis']['events']
                result['fixture_verified'] = self.fixture.returncode == 0 and len(events) == 2 and all(abs(event['dominant_hz'] - 1000) <= 10 for event in events)
                if not result['fixture_verified']:
                    result['status'] = 'inconclusive'
        elif self.process.returncode == 0:
            previews = []
            samples = self.config.get('samples_s', (0, self.duration / 2, max(0, self.duration - .2)))
            for index, second in enumerate(samples):
                target = self.output / f'camera-{index}.png'
                command(['ffmpeg', '-nostdin', '-loglevel', 'error', '-y', '-ss', str(second), '-i', str(self.path),
                         '-frames:v', '1', '-update', '1', str(target)])
                if target.exists():
                    previews.append(str(target))
            result['representative_frames'] = previews
            result['observation'] = 'Capture succeeded; physical interpretation requires image inspection.'
        return result
