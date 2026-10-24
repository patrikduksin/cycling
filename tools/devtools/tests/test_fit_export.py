"""FIT files from the firmware encoder, checked with Garmin's FIT SDK and fitdecode.

These run the real `ride-fit` host binary on a synthetic ride prefix, so they
cover the Rust encoder end to end with two independent decoders.
"""

import datetime as dt
import json
import subprocess
import tempfile
import unittest
import zlib
from pathlib import Path

import fitdecode
from garmin_fit_sdk import Decoder, Stream

from cycling_devtools.rides.export import COMMIT, SLOT_SIZE, fit_command, write_fit

T0_MS = 1_789_000_000_000
LAT0, LON0 = -333_646_900, -705_155_800


def slot(kind, ride_id, sequence, active_ms, samples=()):
    data = bytearray(b'\xff' * SLOT_SIZE)
    data[:4] = b'RIDE'
    data[4:8] = bytes([1, kind, len(samples), 2])
    data[8:12] = ride_id.to_bytes(4, 'little')
    data[12:16] = sequence.to_bytes(4, 'little')
    data[16:24] = active_ms.to_bytes(8, 'little')
    data[24:26] = (len(samples) * 48).to_bytes(2, 'little')
    data[26:32] = b'\0' * 6
    for index, sample in enumerate(samples):
        at = 32 + index * 48
        data[at : at + 8] = sample['active_ms'].to_bytes(8, 'little')
        flags = 0
        if sample.get('utc_ms') is not None:
            flags |= 1
            data[at + 8 : at + 16] = sample['utc_ms'].to_bytes(8, 'little')
        lat, lon = sample['location']
        flags |= 2
        data[at + 20 : at + 24] = lat.to_bytes(4, 'little', signed=True)
        data[at + 24 : at + 28] = lon.to_bytes(4, 'little', signed=True)
        flags |= 8 | 16 | 64 | 128
        data[at + 32 : at + 34] = sample['heart'].to_bytes(2, 'little')
        data[at + 34 : at + 36] = sample['cadence'].to_bytes(2, 'little')
        data[at + 40 : at + 44] = sample['speed'].to_bytes(4, 'little')
        data[at + 38 : at + 40] = sample['power'].to_bytes(2, 'little')
        if sample.get('altitude_m') is not None:
            data[at + 46 : at + 48] = round((sample['altitude_m'] + 500) * 5).to_bytes(2, 'little')
        data[at + 16 : at + 20] = flags.to_bytes(4, 'little')
    data[28:32] = zlib.crc32(data[:252]).to_bytes(4, 'little')
    data[252:256] = COMMIT.to_bytes(4, 'little')
    return bytes(data)


def ride(ride_id=1, seconds=600, pause_at=300, pause_wall_ms=120_000, utc=True, utc_from_s=1):
    """A ride at 1 Hz heading north at 8 m/s, paused once. Its UTC starts at second `utc_from_s`."""
    slots, sequence, batch = [slot(1, ride_id, 0, 0)], 1, []

    def flush():
        nonlocal sequence
        if batch:
            slots.append(slot(2, ride_id, sequence, batch[-1]['active_ms'], batch))
            sequence += 1
            batch.clear()

    for second in range(1, seconds + 1):
        active = second * 1000
        wall = active + (pause_wall_ms if second > pause_at else 0)
        batch.append(
            {
                'active_ms': active,
                'utc_ms': T0_MS + wall if utc and second >= utc_from_s else None,
                'location': (LAT0 + second * 720, LON0),
                'heart': 120 + second % 40,
                'cadence': 850 + second % 100,
                'speed': 8_000,
                'power': 150 + (second % 60) * 2,
                # Up 0.2 m a second to the pause, then back down.
                'altitude_m': 100 + 0.2 * min(second, 2 * pause_at - second),
            }
        )
        if len(batch) == 4:
            flush()
        if second == pause_at:
            flush()
            slots.append(slot(3, ride_id, sequence, active))
            slots.append(slot(4, ride_id, sequence + 1, active))
            sequence += 2
    flush()
    slots.append(slot(5, ride_id, sequence, seconds * 1000))
    return slots


def debug_build(command, **kwargs):
    """The test reuses the debug build `mise run test` already made."""
    return subprocess.run([part for part in command if part != '--release'], **kwargs)


class FitExportTests(unittest.TestCase):
    def convert(self, slots, start_utc_ms=None):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        directory = Path(temporary.name)
        raw = directory / 'ride-slots.bin'
        raw.write_bytes(b''.join(slots))
        rides = write_fit(directory, raw, start_utc_ms, run=debug_build)
        return directory, rides

    def test_garmin_sdk_reads_a_valid_cycling_activity(self):
        directory, rides = self.convert(ride() + ride(ride_id=2, seconds=40, pause_at=20))
        self.assertEqual([ride['ride_id'] for ride in rides], [1, 2])
        self.assertEqual(json.loads((directory / 'fit.json').read_text())['rides'], rides)
        data = (directory / 'ride-1.fit').read_bytes()
        self.assertEqual(rides[0]['bytes'], len(data))
        stream = Stream.from_byte_array(bytearray(data))
        decoder = Decoder(stream)
        self.assertTrue(decoder.is_fit())
        self.assertTrue(decoder.check_integrity())
        decoder = Decoder(Stream.from_byte_array(bytearray(data)))
        messages, errors = decoder.read(convert_datetimes_to_dates=True)
        self.assertEqual(errors, [])
        file_id = messages['file_id_mesgs'][0]
        self.assertEqual(file_id['type'], 'activity')
        start = dt.datetime.fromtimestamp(T0_MS / 1000, dt.UTC)
        self.assertEqual(file_id['time_created'], start)
        records = messages['record_mesgs']
        self.assertEqual(len(records), 600)
        times = [record['timestamp'] for record in records]
        self.assertEqual(times, sorted(times))
        self.assertEqual(times[0] - start, dt.timedelta(seconds=1))
        self.assertEqual(times[-1] - start, dt.timedelta(seconds=720), 'the pause keeps its wall time')
        first = records[0]
        self.assertAlmostEqual(first['position_lat'] * 180 / 2**31, (LAT0 + 720) / 1e7, places=6)
        self.assertAlmostEqual(first['position_long'] * 180 / 2**31, LON0 / 1e7, places=6)
        self.assertEqual(first['speed'], 8.0)
        self.assertEqual(first['heart_rate'], 121)
        self.assertEqual(first['power'], 152)
        self.assertAlmostEqual(records[-1]['distance'], 4800.0)
        self.assertAlmostEqual(first['altitude'], 100.2)
        self.assertAlmostEqual(first['enhanced_altitude'], 100.2)
        self.assertAlmostEqual(records[299]['altitude'], 160.0)
        self.assertAlmostEqual(records[-1]['altitude'], 100.0)
        events = [(event['event'], event['event_type']) for event in messages['event_mesgs']]
        self.assertEqual(events, [('timer', 'start'), ('timer', 'stop_all'), ('timer', 'start'), ('timer', 'stop_all')])
        session = messages['session_mesgs'][0]
        self.assertEqual(session['sport'], 'cycling')
        self.assertEqual(session['start_time'], start)
        self.assertEqual(session['total_timer_time'], 600.0)
        self.assertEqual(session['total_elapsed_time'], 720.0)
        self.assertAlmostEqual(session['total_distance'], 4800.0)
        self.assertEqual(session['num_laps'], 1)
        self.assertGreater(session['normalized_power'], session['avg_power'])
        self.assertEqual((session['total_ascent'], session['total_descent']), (60, 60))
        self.assertEqual(len(messages['lap_mesgs']), 1)
        self.assertEqual(
            (messages['lap_mesgs'][0]['total_ascent'], messages['lap_mesgs'][0]['total_descent']), (60, 60)
        )
        activity = messages['activity_mesgs'][0]
        self.assertEqual(activity['num_sessions'], 1)

    def test_fitdecode_checks_crc_and_every_record(self):
        directory, _ = self.convert(ride(seconds=120, pause_at=60))
        names = []
        with fitdecode.FitReader(
            directory / 'ride-1.fit', check_crc=fitdecode.CrcCheck.RAISE, error_handling=fitdecode.ErrorHandling.RAISE
        ) as reader:
            for frame in reader:
                if isinstance(frame, fitdecode.FitDataMessage):
                    names.append(frame.name)
                    self.assertEqual([field.name for field in frame.fields if field.name.startswith('unknown')], [])
        self.assertEqual(names[:2], ['file_id', 'device_info'])
        self.assertEqual(names[-3:], ['lap', 'session', 'activity'])
        self.assertEqual(names.count('record'), 120)

    def test_rides_without_a_clock_need_a_start(self):
        _, rides = self.convert(ride(utc=False, seconds=20, pause_at=10))
        self.assertIsNone(rides[0]['fit'])
        self.assertIn('--start-utc-ms', rides[0]['fit_error'])
        directory, rides = self.convert(ride(utc=False, seconds=20, pause_at=10), start_utc_ms=T0_MS)
        self.assertFalse(rides[0]['dated'])
        messages, errors = Decoder(Stream.from_file(str(directory / 'ride-1.fit'))).read()
        self.assertEqual(errors, [])
        self.assertEqual(messages['session_mesgs'][0]['total_elapsed_time'], 20.0)

    def test_a_clock_found_part_way_dates_the_start(self):
        def start_and_first_record(slots):
            directory, rides = self.convert(slots)
            self.assertTrue(rides[0]['dated'])
            path = directory / 'ride-1.fit'
            messages, errors = Decoder(Stream.from_file(str(path))).read(convert_datetimes_to_dates=True)
            self.assertEqual(errors, [])
            with fitdecode.FitReader(path, check_crc=fitdecode.CrcCheck.RAISE) as reader:
                frames = [frame for frame in reader if isinstance(frame, fitdecode.FitDataMessage)]
            session = next(frame for frame in frames if frame.name == 'session')
            record = next(frame for frame in frames if frame.name == 'record')
            self.assertEqual(session.get_value('start_time'), messages['session_mesgs'][0]['start_time'])
            self.assertEqual(record.get_value('timestamp'), messages['record_mesgs'][0]['timestamp'])
            return session.get_value('start_time'), record.get_value('timestamp')

        dated = start_and_first_record(ride())
        late = start_and_first_record(ride(utc_from_s=61))
        self.assertEqual(late, dated, 'the clock arriving at second 61 still dates the first second')
        start = dt.datetime.fromtimestamp(T0_MS / 1000, dt.UTC)
        self.assertEqual(late[0].astimezone(dt.UTC), start)
        self.assertEqual(late[1] - late[0], dt.timedelta(seconds=1))

    def test_command_uses_the_firmware_encoder(self):
        command = fit_command(Path('in.bin'), Path('out'), 5)
        self.assertEqual(command[command.index('--bin') + 1], 'ride-fit')
        # Absolute, because cargo runs from the workspace root.
        here = Path.cwd()
        self.assertEqual(command[-5:], ['--', str(here / 'in.bin'), str(here / 'out'), '--start-utc-ms', '5'])


if __name__ == '__main__':
    unittest.main()
