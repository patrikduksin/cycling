import json
import shutil
import subprocess

import pytest

from cycling_devtools.workspace import ROOT

PAGE = ROOT / 'apps/vana/web/src/index.html'
RUNNER = """
const fs = require('fs');
const html = fs.readFileSync(process.argv[1], 'utf8');
const source = html.slice(html.indexOf('function el('), html.indexOf("let showing = '';"));
const { status, rides, click } = JSON.parse(fs.readFileSync(0, 'utf8'));

class Node {}
class Text extends Node {
  constructor(text) { super(); this.text = text; }
  get textContent() { return this.text; }
}
class Element extends Node {
  constructor(tag) {
    super();
    this.tagName = tag.toUpperCase();
    this.attributes = {};
    this.children = [];
    this.listeners = {};
    this.className = '';
    this.classList = { toggle: () => {} };
  }
  setAttribute(key, value) { this.attributes[key] = String(value); }
  removeAttribute(key) { delete this.attributes[key]; }
  get href() { return this.attributes.href; }
  set href(value) { this.attributes.href = value; }
  addEventListener(type, listener) { this.listeners[type] = listener; }
  append(...nodes) { this.children.push(...nodes); }
  replaceChildren(...nodes) { this.children = nodes; }
  remove() {}
  click() { downloads.push({ href: this.href, download: this.attributes.download }); }
  get textContent() { return this.children.map((child) => child.textContent).join(''); }
  set textContent(text) { this.children = [new Text(text)]; }
  *walk() { yield this; for (const child of this.children) if (child instanceof Element) yield* child.walk(); }
}

const downloads = [];
const requests = [];
const elements = { status: new Element('div'), list: new Element('div'), foot: new Element('span') };
const document = { createElement: (tag) => new Element(tag), createTextNode: (text) => new Text(text), body: new Element('body') };
const answers = {
  '/api/status': status,
  '/api/activities': { indexing: false, activities: rides },
  '/api/synced': null,
};
const api = async (path, options = {}) => {
  requests.push(`${options.method || 'GET'} ${path.split('?')[0]}`);
  if (!(path.split('?')[0] in answers)) throw new Error('not here');
  return answers[path.split('?')[0]];
};
const fetch = async (url) => {
  requests.push(`FETCH ${url}`);
  // A FIT header, a little record data, and the trailer the page checks.
  const data = Uint8Array.from([14, 32, ...new Array(6).fill(0), 46, 70, 73, 84, 0, 0, 1, 2, 3, 4]);
  const crc = page.fitCrc(data);
  const body = Uint8Array.from([...data, crc & 0xff, crc >> 8]);
  return {
    status: 200,
    ok: true,
    headers: { get: (name) => (name === 'X-Receipt' ? '7' : null) },
    arrayBuffer: async () => body.buffer,
  };
};
const URL = { createObjectURL: () => 'blob:ride', revokeObjectURL: () => {} };
class Blob {}
const page = new Function(
  'document', 'Node', 'api', 'fetch', 'URL', 'Blob', 'token', '$', 'location', 'setTimeout',
  source + '\\nreturn { load, fitCrc };',
)(document, Node, api, fetch, URL, Blob, 'token', (id) => elements[id], { hash: '' }, () => {});

const saves = (root) => [...root.walk()].filter((node) => ['A', 'BUTTON'].includes(node.tagName) && node.textContent === 'Download .fit');
const describe = (node) => ({ tag: node.tagName, href: node.href || null, download: node.attributes.download ?? null });

(async () => {
  await page.load();
  const list = elements.list;
  const shown = saves(list).map(describe);
  const clear = [...list.walk()].some((node) => node.textContent === 'Clear rides');
  const notes = [];
  for (let n = 0; n < click; n += 1) {
    const button = saves(list)[0];
    await button.listeners.click();
    await new Promise((resume) => setImmediate(resume));
    notes.push([...list.walk()].filter((node) => node.className.startsWith('note') && node.textContent).map((node) => node.textContent).join(' | '));
  }
  process.stdout.write(JSON.stringify({ shown, clear, requests, downloads, notes }));
})();
"""


def ride(**fields):
    return {
        'id': 'i1-1',
        'ride': 1,
        'medium': 'internal',
        'start': 1792249200,
        'duration_s': 9,
        'distance_m': None,
        'gps': False,
        'power': False,
        'heart': True,
        'end': 'finished',
        'unsaved': False,
        **fields,
    }


def page(status, rides, click=0):
    node = shutil.which('node')
    if node is None:
        pytest.skip('node is not installed')
    done = subprocess.run(
        [node, '-e', RUNNER, str(PAGE)],
        input=json.dumps({'status': status, 'rides': rides, 'click': click}),
        text=True,
        capture_output=True,
        check=True,
    )
    return json.loads(done.stdout)


VOLATILE = {'volatile': True, 'ftp': 250, 'lthr': 170}
DURABLE = {'volatile': False, 'ftp': 250, 'lthr': 170}
LINK = {'tag': 'A', 'href': '/api/activities/i1-1.fit', 'download': ''}


def test_a_durable_device_offers_no_clear_and_plain_downloads():
    run = page(DURABLE, [ride()])
    assert not run['clear']
    assert run['shown'] == [LINK]


def test_a_volatile_device_offers_clear_rides():
    assert page(VOLATILE, [ride()])['clear']
    assert not page(VOLATILE, [])['clear'], 'nothing to clear'


def test_a_synced_ride_in_memory_downloads_like_any_other():
    run = page(VOLATILE, [ride(unsaved=False)])
    assert run['shown'] == [LINK]


def test_an_unsaved_ride_is_kept_once_and_then_downloads_plainly():
    run = page(VOLATILE, [ride(unsaved=True)], click=2)
    assert run['shown'] == [{'tag': 'BUTTON', 'href': None, 'download': None}]
    assert [r for r in run['requests'] if not r.startswith('GET')] == [
        'FETCH /api/activities/i1-1.fit',
        'POST /api/synced',
    ], 'the second download is not acknowledged again'
    assert run['downloads'] == [
        {'href': 'blob:ride', 'download': 'i1-1.fit'},
        {'href': '/api/activities/i1-1.fit', 'download': ''},
    ]
    assert run['notes'] == ['Saved to this phone. VANA no longer needs to keep it.'] * 2
    assert all('VANA keeps the ride' not in note for note in run['notes'])
