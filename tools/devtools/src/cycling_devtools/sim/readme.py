"""Render the README's device screenshots from simulator checkpoints.

Each shot is a checkpoint of a ride scenario. The scenarios run with
`SIM_ARTIFACTS` set, and the panel is cut out of each captioned checkpoint
frame and written to `docs/screenshots/`. Run `cycling readme-screenshots`
after a change to any of these screens.
"""

import os
import shutil
import struct
import subprocess
import sys
import zlib

from cycling_devtools.sim import compare
from cycling_devtools.workspace import ROOT

OUT = ROOT / 'docs/screenshots'
ARTIFACTS = ROOT / '.local/sim/readme'

# The panel sits at MARGIN, drawn at SCALE (tools/simulator/src/recording.rs).
MARGIN, SCALE = 20, 2
WIDTH, HEIGHT = 240 * SCALE, 320 * SCALE

# (file in docs/screenshots, scenario, checkpoint file in its artifacts)
SHOTS = [
    ('home.png', 'home_shows_ride_sensors_and_the_last_ride_at_a_glance', '03-home-after-a-ride.png'),
    ('ride.png', 'ride_shows_live_metrics_and_records_the_gps_track', '01-five-minutes-riding.png'),
    (
        'map.png',
        'a_course_from_the_phone_is_followed_on_the_map_off_and_back_on_track',
        '01-the-map-with-the-course-ahead-in-grey.png',
    ),
    ('climb.png', 'climb_page_measures_a_six_percent_climb_over_the_top', '02-climb-page-mid-climb.png'),
    ('radar.png', 'radar_tracks_a_group_of_six_vehicles_until_they_pass', '01-six-vehicles-behind.png'),
    ('workout.png', 'a_structured_workout_runs_its_steps_with_targets_beeps_and_laps', '05-interval-on-target.png'),
    ('summary.png', 'ascent_counts_the_climbs_of_a_hilly_loop', '04-summary-overview-after-the-hilly-lap.png'),
    (
        'light.png',
        'the_light_theme_turns_every_screen_white_and_keeps_its_colours',
        '03-ride-page-in-the-light-theme.png',
    ),
]


def panel(data):
    """The panel of a checkpoint frame as an RGB PNG."""
    decoded = compare.pixels(data)
    if not isinstance(decoded, tuple):
        raise ValueError('checkpoint is not an 8-bit non-interlaced PNG')
    header, rows = decoded
    width, _, _, color, *_ = struct.unpack('>IIBBBBB', header)
    if color != 2:
        raise ValueError('checkpoint is not RGB')
    stride = width * 3
    cropped = b''.join(
        b'\0' + rows[y * stride + MARGIN * 3 : y * stride + (MARGIN + WIDTH) * 3]
        for y in range(MARGIN, MARGIN + HEIGHT)
    )

    def chunk(kind, body):
        return struct.pack('>I', len(body)) + kind + body + struct.pack('>I', zlib.crc32(kind + body))

    return (
        compare.PNG
        + chunk(b'IHDR', struct.pack('>IIBBBBB', WIDTH, HEIGHT, 8, 2, 0, 0, 0))
        + chunk(b'IDAT', zlib.compress(cropped, 9))
        + chunk(b'IEND', b'')
    )


def main():
    shutil.rmtree(ARTIFACTS, ignore_errors=True)
    ARTIFACTS.mkdir(parents=True)
    scenarios = sorted({scenario for _, scenario, _ in SHOTS})
    subprocess.run(
        [*compare.COMMAND, '--', '--exact', *scenarios],
        cwd=ROOT,
        env={**os.environ, 'SIM_ARTIFACTS': str(ARTIFACTS)},
        check=True,
    )
    OUT.mkdir(parents=True, exist_ok=True)
    for name, scenario, checkpoint in SHOTS:
        (OUT / name).write_bytes(panel((ARTIFACTS / scenario / checkpoint).read_bytes()))
        print(f'docs/screenshots/{name}')


if __name__ == '__main__':
    sys.exit(main())
