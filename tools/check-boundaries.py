"""Enforce package ownership, including code disabled by the active Cargo features.

Cargo checks types; this check restricts dependency directions and source routing.
Keep Rust source in ordinary modules: path attributes and include! would let a
package compile another owner's files without declaring that dependency.
The ANT host test has two explicit source seams to substitute UART and locking.
"""
from pathlib import Path
import re
import tomllib

ROOT = Path(__file__).resolve().parents[1]
OWNERS = {
    "device-api": ("packages/device-api", set()),
    "firmware-services": ("packages/services", {"device-api"}),
    "firmware-shell": ("packages/shell", {"device-api", "firmware-services"}),
    "firmware-console": ("packages/console", {"device-api", "firmware-services", "firmware-shell"}),
    "vana": ("apps/vana", {"device-api", "firmware-services", "firmware-shell"}),
    "c606-firmware": ("devices/c606", {"device-api", "firmware-services", "firmware-shell", "firmware-console", "vana"}),
    "shell-simulator": ("tools/simulator", {"device-api", "firmware-services", "firmware-shell", "firmware-console", "vana"}),
}
HARDWARE = {"esp_hal", "esp32s3", "esp_radio", "esp_rtos", "esp_storage", "esp_alloc", "esp_println", "esp_backtrace", "esp_bootloader_esp_idf", "c606_firmware", "shell_simulator"}
CONSUMERS = {"firmware_shell", "firmware_console", "vana", "shell_simulator"}
DEVICE_COMPOSITION = {"src/main.rs", "src/commands.rs", "src/logging.rs"}


# The host ANT regression compiles the production owner against fake UART and
# locking. These files are also scanned under their strict implementation rules.
TEST_SOURCE_SEAMS = {
    "devices/c606/tests/ant_power.rs": {
        "../src/drivers/ant_protocol.rs", "../src/capabilities/ant.rs",
    },
}

# A Rust lifetime is not a character literal; do not blank code between two
# lifetimes such as &'a and &'b when scanning an import or qualified expression.
STRING = re.compile(
    r"""r(?P<hashes>\#*)".*?"(?P=hashes)|"(?:\\.|[^"\\])*"|'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F_]+\}|.)|[^'\\\n])'""",
    re.S,
)


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


def violations(text, forbidden, portable=False, hardware=False, allowed_paths=()):
    comments_removed = without_comments(text)
    code = STRING.sub(lambda match: blank(match.group()), comments_removed)
    code = code.replace('r#', '  ')
    findings = set()

    def report(offset, reason):
        findings.add((code.count('\n', 0, offset) + 1, reason))

    # Paths catch qualified expressions; imports also catch a grouped leaf or
    # `extern crate name as alias` before the alias hides its original spelling.
    for match in re.finditer(r'\b([a-z_][a-z_0-9]*)\s*::|\bmod\s+([a-z_][a-z_0-9]*)', code):
        name = match[1] or match[2]
        if name in forbidden:
            report(match.start(), name)
    for match in re.finditer(r'\b(?:use|extern\s+crate)\b[^;]*;', code):
        for name in set(re.findall(r'\b[a-z_][a-z_0-9]*\b', match.group())) & forbidden:
            report(match.start(), name)
    for match in re.finditer(r'#\s*\[[^\]]*?\bpath\s*=|\binclude\s*!', code):
        target = re.match(r'\s*"([^"]+)"', comments_removed[match.end():])
        if target and target[1] in allowed_paths:
            continue
        report(match.start(), 'source indirection: use ordinary package modules')

    # Scan balanced cfg expressions, including cfg_attr and cfg!, without
    # selecting the current build's features. String values remain available.
    for match in re.finditer(r'\bcfg(?:_attr)?\s*!?\s*\(', code):
        end, depth = match.end(), 1
        while end < len(code) and depth:
            depth += (code[end] == '(') - (code[end] == ')')
            end += 1
        condition = comments_removed[match.start():end]
        if portable and re.search(r'c606|esp32|magene', condition, re.I):
            report(match.start(), 'board-specific condition in portable code')
        if hardware and re.search(r'feature\s*=\s*"cycling"', condition):
            report(match.start(), 'application feature in hardware implementation')
    return sorted(findings)


def dependencies(manifest, workspace=None):
    """Include aliased, optional, target, development and build dependencies."""
    for key, section in manifest.items():
        if key in {'dependencies', 'dev-dependencies', 'build-dependencies'}:
            for alias, spec in section.items():
                if isinstance(spec, dict) and spec.get('workspace'):
                    spec = (workspace or {}).get(alias, {})
                yield alias, spec.get('package', alias) if isinstance(spec, dict) else alias
        elif isinstance(section, dict):
            yield from dependencies(section, workspace)


def source_rules(owner, relative, aliases):
    if owner == 'c606-firmware':
        composition = relative.as_posix() in DEVICE_COMPOSITION or relative.parts[0] == 'tests'
        forbidden = set() if composition else CONSUMERS | {'main', 'commands', 'logging'}
        portable, hardware = False, not composition
    elif owner == 'shell-simulator':
        forbidden, portable, hardware = {'c606_firmware'}, False, False
    else:
        allowed = OWNERS[owner][1]
        forbidden = HARDWARE | {name.replace('-', '_') for name in OWNERS
                                if name != owner and name not in allowed}
        portable, hardware = True, False
    forbidden |= {alias.replace('-', '_') for alias, package in aliases
                  if package.replace('-', '_') in forbidden}
    return forbidden, portable, hardware


def self_test():
    for source in [
        'use firmware_shell::{Shell};', 'use {vana as app, device_api};',
        'use crate::{nested::{vana as app}};', 'extern crate vana as app;',
        'pub use firmware_shell as shared;', 'r#vana::App::new()', 'fn f<\'a>(v: &\'a vana::App) {}',
        'fn f<\'a, \'b>(v: &\'a str) -> &\'b vana::App { todo!() }',
        '#[cfg(feature = "unused")] use vana::App;',
        '#[path = "../app.rs"] mod innocent;',
        '#[cfg_attr(feature = "unused", path = "../app.rs")] mod innocent;',
        'include!(concat!(env!("OUT_DIR"), "/generated.rs"));',
    ]:
        assert violations(source, CONSUMERS), source
    harmless = '''// use vana::App;
/* nested /* firmware_shell::Shell */ vana::App */
const NAME: &str = "vana::App // text";
const RAW: &str = r##"/* use vana::App; */"##;
use device_api::{display, input};
log::debug!("read failed");
'''
    assert not violations(harmless, CONSUMERS)
    for source in ['#[cfg(feature = "c606")] fn hidden() {}',
                   'cfg!(all(feature = "other", feature = "esp32s3"))',
                   '#[cfg_attr(any(feature = "x", feature = "magene"), inline)] fn f() {}']:
        assert violations(source, HARDWARE, portable=True), source
    assert violations('#[cfg(feature = "cycling")] fn f() {}', CONSUMERS, hardware=True)
    assert not violations('#[path = "owner.rs"] mod owner;', CONSUMERS, allowed_paths={'owner.rs'})
    assert violations('#[path = "other.rs"] mod owner;', CONSUMERS, allowed_paths={'owner.rs'})
    assert violations('include!("owner.rs");', CONSUMERS, allowed_paths={'owner.rs'})
    manifest = tomllib.loads('''
[dependencies]
app = { package = "vana", path = "../../apps/vana", optional = true }
[target.\'cfg(target_os = "none")\'.build-dependencies]
board = { package = "c606-firmware", path = "../../devices/c606" }
[dev-dependencies]
ui = { package = "firmware-shell", path = "../shell" }
''')
    aliases = list(dependencies(manifest))
    assert list(dependencies({'dependencies': {'app': {'workspace': True}}},
                             {'app': {'package': 'vana'}})) == [('app', 'vana')]
    assert set(aliases) == {('app', 'vana'), ('board', 'c606-firmware'), ('ui', 'firmware-shell')}
    rules = source_rules('c606-firmware', Path('src/drivers/display.rs'), aliases)
    assert violations('use app::{Screen};', *rules)
    assert violations('use crate::{commands as dispatch};', *rules)
    assert not violations('use firmware_shell::Shell;', *source_rules('c606-firmware', Path('src/main.rs'), aliases))
    assert violations('use board::Board;', *source_rules('firmware-shell', Path('src/lib.rs'), aliases))


def main():
    self_test()
    failures, paths = [], []
    workspace = tomllib.loads((ROOT / 'Cargo.toml').read_text()).get('workspace', {}).get('dependencies', {})
    for name, (directory, allowed) in OWNERS.items():
        package_root = ROOT / directory
        manifest_path = package_root / 'Cargo.toml'
        manifest = tomllib.loads(manifest_path.read_text())
        aliases = list(dependencies(manifest, workspace))
        for _, dependency in aliases:
            if (dependency in OWNERS and dependency not in allowed
                    or name not in {'c606-firmware', 'shell-simulator'}
                    and dependency.replace('-', '_') in HARDWARE):
                failures.append(f'{manifest_path.relative_to(ROOT)}: prohibited dependency on {dependency}')
        # Include tests, examples and build scripts, not generated target output.
        sources = [path for folder in ('src', 'tests', 'examples', 'benches')
                   for path in (package_root / folder).rglob('*.rs')]
        if (package_root / 'build.rs').exists():
            sources.append(package_root / 'build.rs')
        for path in sorted(sources):
            paths.append(path)
            rules = source_rules(name, path.relative_to(package_root), aliases)
            failures.extend(f'{path.relative_to(ROOT)}:{line}: prohibited dependency on {item}'
                            for line, item in violations(path.read_text(), *rules,
                                allowed_paths=TEST_SOURCE_SEAMS.get(path.relative_to(ROOT).as_posix(), ())))
    if failures:
        raise SystemExit('\n'.join(failures))
    print(f'Dependency boundaries passed for {len(OWNERS)} packages and {len(paths)} Rust files.')


if __name__ == '__main__':
    main()
