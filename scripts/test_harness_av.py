import math
from pathlib import Path
import tempfile
import unittest
from harness_av import analyze_audio, fft, tone_fixture


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
            tone_fixture(path, amplitude=0)
            self.assertEqual(analyze_audio(path)['status'], 'inconclusive')

    def test_fft_dc(self):
        self.assertEqual(fft([1, 1, 1, 1]), [4, 0j, 0j, 0j])
