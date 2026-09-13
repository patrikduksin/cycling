import array
import io
import math
import sys
import wave
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch
from harness_av import Capture, analyze_audio, discover, fft, tone_fixture


class AudioTests(unittest.TestCase):
    def test_known_fixture_onsets_frequency_levels_and_silence(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'tone.wav'
            tone_fixture(path)
            result = analyze_audio(path)
            self.assertEqual(result['status'], 'pass')
            self.assertEqual(result['repetitions'], 2)
            for event, onset in zip(result['events'], (.7, 1.6)):
                self.assertAlmostEqual(event['onset_s'], onset, places=2)
                self.assertAlmostEqual(event['duration_s'], .4, places=2)
                self.assertAlmostEqual(event['dominant_hz'], 1000, delta=4)
                self.assertAlmostEqual(event['rms_dbfs'], 20 * math.log10(.15 / math.sqrt(2)), delta=.1)
            tone_fixture(path, amplitude=1)
            self.assertGreater(analyze_audio(path)['clipped_samples'], 0)
            self.assertEqual(analyze_audio(path)['status'], 'inconclusive')
            tone_fixture(path, amplitude=0)
            self.assertEqual(analyze_audio(path)['status'], 'inconclusive')

    def test_fft_dc(self):
        self.assertEqual(fft([1, 1, 1, 1]), [4, 0j, 0j, 0j])


class CaptureTests(unittest.TestCase):
    def test_camera_selection_uses_v4l2_card_name_when_sysfs_only_reports_driver(self):
        with tempfile.TemporaryDirectory() as directory:
            paths = [Path(directory) / name for name in ('video0', 'video1')]
            for path in paths:
                path.mkdir()
                (path / 'name').write_text('apple-isp\n')
            calls = []
            def command(args):
                calls.append(args)
                if args[-1] == '--get-fmt-video':
                    return "Width/Height : 1280/720\nPixel Format : 'NV12'\n"
                if args[-1] == '--list-ctrls':
                    return 'exposure: current=100 default=100'
                self.assertEqual(args[-1], '--all')
                card = 'FaceTime HD Camera' if args[2] == '/dev/video0' else 'Apple ISP Auxiliary'
                return f'Driver name : apple-isp\nCard type : {card}\nDevice Caps : Video Capture\n'
            with patch('harness_av.Path.glob', return_value=paths), \
                 patch('harness_av.command', side_effect=command), \
                 patch('harness_av.shutil.which', side_effect=lambda name: '/fake/ffmpeg' if name == 'ffmpeg' else None):
                inventory = discover()
                self.assertEqual(inventory['cameras'][0]['name'], 'FaceTime HD Camera')
                self.assertTrue(inventory['cameras'][0]['video_capture'])
                capture = Capture('camera', {}, Path(directory) / 'capture')
                setup = capture.preflight()
            self.assertEqual(setup['path'], '/dev/video0')
            self.assertEqual(setup['name'], 'FaceTime HD Camera')
            self.assertIn(['v4l2-ctl', '-d', '/dev/video0', '--list-ctrls'], calls)
            self.assertTrue(all(args[-1] in ('--all', '--list-ctrls', '--get-fmt-video') for args in calls))

    def test_fixture_verification_separates_other_tones_and_rejects_extra_matching_tone(self):
        for frequency, expected_status in ((300, 'pass'), (1000, 'inconclusive')):
            with self.subTest(ambient_frequency=frequency), tempfile.TemporaryDirectory() as directory:
                capture = Capture('microphone', {'fixture': True}, Path(directory))
                tone_fixture(capture.path)
                with wave.open(str(capture.path), 'rb') as stream:
                    parameters = stream.getparams()
                    samples = array.array('h', stream.readframes(stream.getnframes()))
                if sys.byteorder != 'little':
                    samples.byteswap()
                for i in range(round(2.3 * parameters.framerate), round(2.7 * parameters.framerate)):
                    samples[i] = round(32767 * .1 * math.sin(2 * math.pi * frequency * i / parameters.framerate))
                if sys.byteorder != 'little':
                    samples.byteswap()
                with wave.open(str(capture.path), 'wb') as stream:
                    stream.setparams(parameters)
                    stream.writeframes(samples.tobytes())
                capture.process = Mock(returncode=0)
                capture.process.poll.return_value = 0
                capture.fixture = Mock(returncode=0)
                capture.fixture.poll.return_value = 0
                capture.started, capture.log = 0, io.BytesIO()
                capture.setup = {'source': 'deterministic synthetic file'}
                with patch('harness_av.time.monotonic', return_value=0):
                    result = capture.finish()
                self.assertEqual(result['analysis']['repetitions'], 3)
                self.assertEqual(result['analysis']['clipped_samples'], 0)
                self.assertEqual(result['status'], expected_status)
                self.assertEqual(result['fixture_verified'], frequency == 300)
                self.assertEqual(result['other_audio_events'], 1 if frequency == 300 else 0)
                self.assertTrue(capture.log.closed)
                capture.process.wait.assert_called_once()
                capture.fixture.wait.assert_called_once()


    def test_camera_format_restored_after_exit_including_failed_and_partial_start(self):
        before = "Width/Height : 1280/720\nPixel Format : 'NV12'\n"
        changed = "Width/Height : 640/480\nPixel Format : 'YUYV'\n"
        for mode in ('unchanged', 'changed', 'capture-failed', 'partial-start', 'write-failed', 'readback-mismatch', 'lost-write-reply'):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as directory:
                capture = Capture('camera', {'samples_s': []}, Path(directory))
                inventory = {'ffmpeg': True, 'cameras': [{'path': '/dev/fake', 'name': 'Camera', 'video_capture': True}]}
                with patch('harness_av.discover', return_value=inventory), patch('harness_av.command', side_effect=['controls', before]):
                    capture.preflight()
                capture.started, capture.log = 0, io.BytesIO()
                if mode != 'partial-start':
                    capture.process = Mock(returncode=1 if mode == 'capture-failed' else 0)
                    capture.process.poll.return_value = capture.process.returncode
                current = before if mode == 'unchanged' else changed
                calls = []
                def command(args):
                    nonlocal current
                    calls.append(args)
                    if capture.process is not None:
                        capture.process.wait.assert_called_once()
                    if args[-1] == '--get-fmt-video':
                        return current
                    self.assertEqual(args[-1], '--set-fmt-video=width=1280,height=720,pixelformat=NV12')
                    if mode == 'write-failed':
                        raise OSError('camera format write rejected')
                    if mode != 'readback-mismatch':
                        current = before
                    if mode == 'lost-write-reply':
                        raise TimeoutError('write completed but reply lost')
                    return ''
                with patch('harness_av.command', side_effect=command), patch('harness_av.time.monotonic', return_value=0):
                    result = capture.finish()
                uncertain = mode in ('write-failed', 'readback-mismatch')
                restoration = result['format_restoration']
                self.assertEqual(restoration['status'], 'uncertain' if uncertain else 'unchanged' if mode == 'unchanged' else 'verified')
                self.assertIsNone(result['settings_changed'])
                if uncertain:
                    self.assertEqual(result['status'], 'inconclusive')
                elif mode == 'capture-failed':
                    self.assertEqual(result['status'], 'fail')
                else:
                    self.assertEqual(result['status'], 'skipped' if mode == 'partial-start' else 'pass')
                self.assertEqual(sum('--set-fmt-video=' in args[-1] for args in calls), 0 if mode == 'unchanged' else 1)
                self.assertTrue(capture.log.closed)

    def test_camera_format_snapshot_failure_prevents_start(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = Capture('camera', {}, Path(directory))
            inventory = {'ffmpeg': True, 'cameras': [{'path': '/dev/fake', 'name': 'Camera', 'video_capture': True}]}
            with patch('harness_av.discover', return_value=inventory), patch('harness_av.command', side_effect=['controls', 'unrecognized format']):
                with self.assertRaisesRegex(ValueError, 'cannot snapshot camera'):
                    capture.preflight()
            self.assertIsNone(capture.started)
            self.assertIsNone(capture.process)
