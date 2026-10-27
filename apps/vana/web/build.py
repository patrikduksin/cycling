# /// script
# requires-python = ">=3.14"
# dependencies = []
# ///
"""Build the phone web client the device serves: one pre-gzipped page.

`src/index.html` holds the whole client (markup, style and script inline), so
the device answers one request with one flash-resident file. The output is
deterministic (no timestamp, fixed level) and committed, so firmware builds
need no web toolchain. `--check` fails when the committed file is stale.
"""

import gzip
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
SOURCE = HERE / 'src' / 'index.html'
OUTPUT = HERE / 'dist' / 'index.html.gz'
# The device keeps this in flash; the budget leaves room for the firmware.
BUDGET = 200 * 1024


def build():
    return gzip.compress(SOURCE.read_bytes(), compresslevel=9, mtime=0)


def main():
    data = build()
    if len(data) > BUDGET:
        raise SystemExit(f'web client is {len(data)} bytes gzipped, over the {BUDGET} byte budget')
    if '--check' in sys.argv[1:]:
        if not OUTPUT.exists() or OUTPUT.read_bytes() != data:
            raise SystemExit(f'{OUTPUT.relative_to(HERE.parents[2])} is stale: run cycling web')
        return
    OUTPUT.parent.mkdir(exist_ok=True)
    OUTPUT.write_bytes(data)
    print(f'{OUTPUT.relative_to(HERE.parents[2])}: {len(data)} bytes gzipped from {SOURCE.stat().st_size}')


if __name__ == '__main__':
    main()
