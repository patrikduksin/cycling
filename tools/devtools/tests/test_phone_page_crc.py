import json
import random
import shutil
import subprocess

import pytest

from cycling_devtools.workspace import ROOT

PAGE = ROOT / 'apps/vana/web/src/index.html'
RUNNER = """
const fs = require('fs');
const html = fs.readFileSync(process.argv[1], 'utf8');
const start = html.indexOf('const FIT_CRC');
const end = html.indexOf('async function keepRide');
const fitCrc = new Function(html.slice(start, end) + '\\nreturn fitCrc;')();
const out = JSON.parse(fs.readFileSync(0, 'utf8')).map((bytes) => fitCrc(Uint8Array.from(bytes)));
process.stdout.write(JSON.stringify(out));
"""

# FIT protocol, "Cyclic Redundancy Check": a nibble table, low nibble first.
TABLE = (0x0000, 0xCC01, 0xD801, 0x1400, 0xF001, 0x3C00, 0x2800, 0xE401,
         0xA001, 0x6C00, 0x7800, 0xB401, 0x5000, 0x9C01, 0x8801, 0x4400)  # fmt: skip


def fit_crc(data):
    crc = 0
    for byte in data:
        for nibble in (byte & 0xF, byte >> 4):
            crc = ((crc >> 4) & 0x0FFF) ^ TABLE[crc & 0xF] ^ TABLE[nibble]
    return crc


def page_crc(cases):
    node = shutil.which('node')
    if node is None:
        pytest.skip('node is not installed')
    done = subprocess.run(
        [node, '-e', RUNNER, str(PAGE)], input=json.dumps(cases), text=True, capture_output=True, check=True
    )
    return json.loads(done.stdout)


def test_the_page_computes_fits_crc_and_a_whole_file_checks_out():
    rng = random.Random(398)
    bodies = [b'', b'.FIT', bytes(range(256))] + [rng.randbytes(rng.randrange(1, 600)) for _ in range(20)]
    assert page_crc([list(body) for body in bodies]) == [fit_crc(body) for body in bodies]
    body = rng.randbytes(300)
    whole = body + fit_crc(body).to_bytes(2, 'little')
    assert page_crc([list(whole)]) == [0], 'a file with its own CRC trailer checks to zero'
