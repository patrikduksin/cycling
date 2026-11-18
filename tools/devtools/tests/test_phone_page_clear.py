import json
import shutil
import subprocess

import pytest

from cycling_devtools.workspace import ROOT

PAGE = ROOT / 'apps/vana/web/src/index.html'
RUNNER = """
const fs = require('fs');
const html = fs.readFileSync(process.argv[1], 'utf8');
const source = html.slice(html.indexOf('async function clearRides'), html.indexOf('function clearRow'));
const answers = JSON.parse(fs.readFileSync(0, 'utf8'));
const seen = [];
const requests = [];
const button = { disabled: false };
const note = { className: '', textContent: '' };
let reloaded = false;
const api = async (path, options = {}) => {
  requests.push(`${options.method || 'GET'} ${path}`);
  seen.push({ note: note.textContent, disabled: button.disabled });
  const answer = answers.shift();
  if (answer.error) throw new Error(answer.error);
  return answer;
};
const setTimeout = (resume) => resume();
const load = () => { reloaded = true; };
const clearRides = new Function('api', 'setTimeout', 'load', source + '\\nreturn clearRides;')(api, setTimeout, load);
clearRides(button, note).then(() => {
  process.stdout.write(JSON.stringify({ seen, requests, reloaded, button, note }));
});
"""


def clear_rides(*answers):
    node = shutil.which('node')
    if node is None:
        pytest.skip('node is not installed')
    done = subprocess.run(
        [node, '-e', RUNNER, str(PAGE)], input=json.dumps(answers), text=True, capture_output=True, check=True
    )
    return json.loads(done.stdout)


def test_the_page_waits_on_the_device_and_back_keeps_the_rides():
    run = clear_rides({'clear': 'asking'}, {'clear': 'asking'}, {'clear': 'kept'})
    assert run['requests'] == ['DELETE /api/activities', 'GET /api/status', 'GET /api/status']
    assert run['seen'][1:] == [{'note': 'Confirm on the device.', 'disabled': True}] * 2
    assert run['note'] == {'className': 'note', 'textContent': 'Rides kept.'}
    assert run['button'] == {'disabled': False}
    assert not run['reloaded']


def test_a_clear_reloads_the_list():
    run = clear_rides({'clear': 'asking'}, {'clear': 'clearing'}, {'clear': 'cleared'})
    assert run['reloaded']
    synced = clear_rides({'clear': 'clearing'}, {'clear': 'cleared'})
    assert synced['reloaded'] and synced['seen'][1]['note'] == 'Clearing…'
    read_by_the_host = clear_rides({'clear': 'asking'}, {'clear': None})
    assert read_by_the_host['reloaded'], 'the list shows what happened'


def test_errors_use_the_note_and_bring_the_button_back():
    refused = clear_rides({'error': 'Device answered 409'})
    assert refused['note'] == {'className': 'note alert', 'textContent': 'Device answered 409'}
    assert refused['button'] == {'disabled': False}
    failed = clear_rides({'clear': 'clearing'}, {'clear': 'failed'})
    assert failed['note']['textContent'] == 'VANA could not clear its rides.'
    assert not failed['reloaded']
