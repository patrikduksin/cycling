"""Check source dependency direction; cargo base/cycling checks prove feature isolation."""
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'packages/os/src'
# These modules compose consumers. All other top-level modules and every device/
# and services/ module must remain usable without the cycling SDK or old shell.
COMPOSITION = {'lib.rs', 'main.rs', 'terminal.rs', 'sdk_runtime.rs'}
FORBIDDEN = {'sdk', 'sdk_runtime', 'ride', 'ride_log', 'ride_reclaim', 'ride_recorder',
             'ble_sensor', 'ui', 'coin', 'controls', 'debug', 'debug_usb', 'metrics',
             'redraw', 'screenshot'}
STRING = re.compile(r'r(?P<hashes>\#*)".*?"(?P=hashes)|"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])\'', re.S)


def blank(text):
    return ''.join('\n' if char == '\n' else ' ' for char in text)


def without_comments(text):
    """Retain literals, so comment markers inside Rust strings are harmless."""
    result, index = [], 0
    while index < len(text):
        literal = STRING.match(text, index)
        if literal:
            result.append(literal.group())
            index = literal.end()
        elif text.startswith('//', index):
            end = text.find('\n', index)
            end = len(text) if end < 0 else end
            result.append(blank(text[index:end]))
            index = end
        elif text.startswith('/*', index):
            end, depth = index + 2, 1
            while end < len(text) and depth:
                if text.startswith('/*', end):
                    depth += 1
                    end += 2
                elif text.startswith('*/', end):
                    depth -= 1
                    end += 2
                else:
                    end += 1
            result.append(blank(text[index:end]))
            index = end
        else:
            result.append(text[index])
            index += 1
    return ''.join(result)


def violations(text):
    comments_removed = without_comments(text)
    code = STRING.sub(lambda match: blank(match.group()), comments_removed)
    findings = set()
    # Qualified expressions and aliases still reveal the prohibited module name.
    for match in re.finditer(r'\b([a-z_][a-z_0-9]*)\s*::|\bmod\s+([a-z_][a-z_0-9]*)', code):
        name = match[1] or match[2]
        if name in FORBIDDEN:
            findings.add((code.count('\n', 0, match.start()) + 1, name))
    # Grouped local imports need no :: after the imported module itself.
    for match in re.finditer(r'\buse\s+(?:::)?(?:crate|self|super|cycling_os)\b[^;]*;', code):
        for name in set(re.findall(r'\b[a-z_][a-z_0-9]*\b', match.group())) & FORBIDDEN:
            findings.add((code.count('\n', 0, match.start()) + 1, name))
    for match in re.finditer(r'#\s*\[\s*path\s*=\s*"([^"]+)"\s*\]', comments_removed):
        parts = set(re.findall(r'[a-z_][a-z_0-9]*', match[1]))
        for name in parts & FORBIDDEN:
            findings.add((code.count('\n', 0, match.start()) + 1, name))
    return sorted(findings)


def self_test():
    for source in ['use cycling_os::{sdk, storage};', 'crate::ride_log::Slot',
                   'pub mod ui;', '#[path = "../sdk/ride.rs"] mod hidden;',
                   'use crate::{sdk as consumer};']:
        assert violations(source), source
    harmless = '''// use crate::sdk;
/* nested /* sdk::Recorder */ ui::App */
const NAME: &str = "sdk::Recorder // text";
const RAW: &str = r##"/* use crate::ui; */"##;
use cycling_os::{storage, gps};
log::debug!("read failed");
'''
    assert not violations(harmless)


def main():
    self_test()
    paths = sorted(path for path in SOURCE.rglob('*.rs')
                   if 'sdk' not in path.relative_to(SOURCE).parts
                   and path.relative_to(SOURCE).as_posix() not in COMPOSITION)
    failures = []
    for path in paths:
        failures.extend(f'{path.relative_to(ROOT)}:{line}: lower layer depends on {name}'
                        for line, name in violations(path.read_text()))
    if failures:
        raise SystemExit('\n'.join(failures))
    print(f'Boundary imports passed for {len(paths)} core/device files; run cargo base/cycling checks too.')


if __name__ == '__main__':
    main()
