"""Check source dependency direction; cargo base/cycling checks prove feature isolation."""
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'packages/os/src'
# These modules compose consumers. All other top-level modules and every device/
# and services/ module must remain usable without cycling SDK or consumer policy.
COMPOSITION = {'lib.rs', 'main.rs', 'simulator/session.rs'}
FORBIDDEN = {'sdk', 'sdk_runtime', 'terminal', 'shell'}
HARDWARE = {'device', 'esp_hal', 'esp32s3', 'esp_radio', 'esp_rtos', 'esp_storage', 'esp_alloc', 'esp_println', 'services', 'c606', 'core_system', 'simulator'}
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


def violations(text, forbidden=FORBIDDEN, portable=False):
    comments_removed = without_comments(text)
    code = STRING.sub(lambda match: blank(match.group()), comments_removed)
    findings = set()
    # Qualified expressions and aliases still reveal the prohibited module name.
    for match in re.finditer(r'\b([a-z_][a-z_0-9]*)\s*::|\bmod\s+([a-z_][a-z_0-9]*)', code):
        name = match[1] or match[2]
        if name in forbidden:
            findings.add((code.count('\n', 0, match.start()) + 1, name))
    # Grouped local imports need no :: after the imported module itself.
    for match in re.finditer(r'\buse\s+(?:::)?(?:crate|self|super|cycling_os)\b[^;]*;', code):
        for name in set(re.findall(r'\b[a-z_][a-z_0-9]*\b', match.group())) & forbidden:
            findings.add((code.count('\n', 0, match.start()) + 1, name))
    for match in re.finditer(r'#\s*\[\s*path\s*=\s*"([^"]+)"\s*\]', comments_removed):
        parts = set(re.findall(r'[a-z_][a-z_0-9]*', match[1]))
        for name in parts & forbidden:
            findings.add((code.count('\n', 0, match.start()) + 1, name))
    if portable:
        for match in re.finditer(r'\b(?:cfg|cfg_attr)!?\s*[\[(][^;]*?(?:c606|esp32|magene)', comments_removed, re.S):
            findings.add((comments_removed.count('\n', 0, match.start()) + 1, 'board condition'))
    return sorted(findings)


def self_test():
    for source in ['use cycling_os::{sdk, storage};', 'crate::sdk::ride_log::Slot',
                   'pub mod terminal;', '#[path = "../sdk/ride.rs"] mod hidden;',
                   'use crate::{sdk as consumer};',
                   '#[cfg(feature = "cycling")] use cycling_os::sdk::recorder;',
                   '#[cfg(feature = "harness")] use crate::terminal;']:
        assert violations(source), source
    harmless = '''// use crate::sdk;
/* nested /* sdk::Recorder */ terminal::Terminal */
const NAME: &str = "sdk::Recorder // text";
const RAW: &str = r##"/* use crate::terminal; */"##;
use cycling_os::{storage, gps};
log::debug!("read failed");
'''
    assert not violations(harmless)
    for source in ['use crate::device::c606;', 'use esp_hal::{gpio};', '#[cfg(feature = "c606")] fn hidden() {}', 'use crate::{device as board};']:
        assert violations(source, HARDWARE, True), source
    assert violations('use crate::shell::preferences;')
    assert not violations('use crate::capabilities::{Display, InputSource};', HARDWARE, True)


def main():
    self_test()
    paths = sorted(SOURCE.rglob('*.rs'))
    failures = []
    for path in paths:
        relative = path.relative_to(SOURCE)
        if relative.as_posix() in COMPOSITION or relative.parts[0] == 'bin':
            continue
        portable = relative.parts[0] in {'shell', 'sdk'} or relative.name in {'terminal.rs', 'sdk_runtime.rs'}
        forbidden = HARDWARE | ({'shell'} if relative.parts[0] == 'sdk' else set()) if portable else FORBIDDEN
        failures.extend(f'{path.relative_to(ROOT)}:{line}: prohibited dependency on {name}'
                        for line, name in violations(path.read_text(), forbidden, portable))
    if failures:
        raise SystemExit('\n'.join(failures))
    print(f'Boundary imports passed for {len(paths)} core/device files; run cargo base/cycling checks too.')


if __name__ == '__main__':
    main()
