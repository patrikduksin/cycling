"""The phone page's workout encoder against `workouts/load.py`.

The page builds the codec file from workout JSON in the browser, as the USB
tool does on the host. Both must accept the same workouts and make the same
bytes, so a workout is never refused by one and stored by the other. The
page's script runs under node; without node on PATH the test is skipped.
"""

import json
import shutil
import subprocess

import pytest

from cycling_devtools.workouts.load import encode
from cycling_devtools.workspace import ROOT

PAGE = ROOT / 'apps/vana/web/src/index.html'
RUNNER = """
const fs = require('fs');
const html = fs.readFileSync(process.argv[1], 'utf8');
const start = html.indexOf('const WORKOUT_ERRORS');
const end = html.indexOf('async function workoutFile');
const encodeWorkout = new Function(html.slice(start, end) + '\\nreturn [encodeWorkout, parseWorkout];')();
const sourceText = JSON.parse('{"a":1.0}', (k, v, c) => (c && typeof v === "number" ? c.source : v)).a;
const out = JSON.parse(fs.readFileSync(0, 'utf8')).map((text) => {
  try {
    return { hex: Buffer.from(encodeWorkout[0](encodeWorkout[1](text))).toString('hex') };
  } catch (error) {
    return { error: error.message };
  }
});
process.stdout.write(JSON.stringify({ sourceText, out }));
"""

STEP = '{"intensity":"work","minutes":5,"ftp":[88,94]}'


def workout(*, name='"T"', steps=STEP, extra=''):
    return f'{{"name":{name},"steps":[{steps}]{extra}}}'


VALID = [
    workout(),
    workout(
        name='"Over \\"Unders\\" \\u00e9"',
        steps='{"intensity":"warmup","minutes":1.5,"ftp":[40,50]},'
        '{"repeat":3,"steps":[{"intensity":"work","seconds":60,"watts":[300,320],"note":"under"},'
        '{"intensity":"recovery","km":0.5,"heart_zone":2}]},'
        '{"intensity":"work","open":true,"ramp":{"start":100,"step":20,"every":60}},'
        '{"intensity":"cooldown","metres":150,"lthr":[60,70]},{"intensity":"work","seconds":30,"cadence":[90,100]}',
    ),
    workout(extra=',"test":"twenty"'),
    workout(extra=',"test":null'),
    workout(steps='{"intensity":"test","open":true,"ramp":{"start":100,"step":20,"every":60}}', extra=',"test":"ramp"'),
    workout(steps='{"intensity":"work","seconds":1e2,"ftp":[88,94]}'),
]
REFUSED = [
    workout(extra=',"test":"constructor"'),
    workout(extra=',"test":"toString"'),
    workout(extra=',"test":1'),
    workout(steps='{"intensity":"work","minutes":"5","ftp":[88,94]}'),
    workout(steps='{"intensity":"work","minutes":true,"ftp":[88,94]}'),
    workout(steps='{"intensity":"work","open":1,"ftp":[88,94]}'),
    workout(steps='{"intensity":"work","minutes":5,"note":null}'),
    workout(steps='{"intensity":"work","minutes":5,"note":5}'),
    workout(steps='{"intensity":"work","minutes":5,"ftp":[true,94]}'),
    workout(steps='{"intensity":"work","minutes":5,"ftp":["88",94]}'),
    workout(steps='{"intensity":"work","minutes":5,"ftp":[88,94],"constructor":1}'),
    workout(steps='{"intensity":"constructor","minutes":5}'),
    workout(steps='{"intensity":"work","minutes":5,"ftp":[88,94],"watts":[100,200]}'),
    workout(steps='{"intensity":"work","minutes":5,"heart_zone":6}'),
    workout(steps='{"intensity":"work","metres":99}'),
    workout(steps='{"intensity":"work","minutes":0}'),
    workout(steps='{"repeat":2,"steps":[]}'),
    workout(steps='{"repeat":"2","steps":[' + STEP + ']}'),
    workout(steps='{"repeat":2,"steps":[' + STEP + '],"extra":1}'),
    workout(name='null'),
    workout(name='"   "'),
    workout(name='"A NAME LONGER THAN TWENTY"'),
    workout(name='"\\u0001"'),
    workout(steps=''),
    workout(steps=','.join([STEP] * 13)),
    '{"name":"X"}',
    '[]',
    'null',
    '{"name":"X","steps":[5]}',
]
# Python reads these numbers as floats and refuses them where it wants a whole
# number; the page can only tell when the browser gives JSON.parse the source.
FLOATS = [
    workout(steps='{"intensity":"work","minutes":5,"ftp":[88.0,94]}'),
    workout(steps='{"repeat":2.0,"steps":[' + STEP + ']}'),
    workout(steps='{"intensity":"work","minutes":5,"ftp":[8.8e1,94]}'),
]


@pytest.fixture(scope='module')
def page():
    node = shutil.which('node')
    if node is None:
        pytest.skip('node is not installed')
    cases = [*VALID, *REFUSED, *FLOATS]
    done = subprocess.run(
        [node, '-e', RUNNER, str(PAGE)], input=json.dumps(cases), text=True, capture_output=True, check=True
    )
    reply = json.loads(done.stdout)
    return dict(zip(cases, reply['out'], strict=True)), reply['sourceText'] == '1.0'


def host(text):
    try:
        return encode(json.loads(text)).hex()
    except ValueError as error:
        return error


@pytest.mark.parametrize('text', VALID)
def test_a_workout_load_py_takes_makes_the_same_bytes_on_the_page(page, text):
    expected = host(text)
    assert isinstance(expected, str), expected
    assert page[0][text] == {'hex': expected}


@pytest.mark.parametrize('text', REFUSED)
def test_a_workout_load_py_refuses_is_refused_by_the_page(page, text):
    assert not isinstance(host(text), str), 'load.py takes it'
    assert 'error' in page[0][text], page[0][text]


@pytest.mark.parametrize('text', FLOATS)
def test_a_float_where_a_whole_number_is_wanted_is_refused_when_the_browser_can_tell(page, text):
    results, can_tell = page
    assert not isinstance(host(text), str), 'load.py takes it'
    if can_tell:
        assert 'error' in results[text], results[text]
