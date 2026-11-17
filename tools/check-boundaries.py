# /// script
# requires-python = ">=3.14"
# dependencies = []
# ///
"""Enforce package ownership, including code disabled by the active Cargo features.

Cargo checks types; this check restricts dependency directions and source routing.
Keep Rust source in ordinary modules: path attributes and include! would let a
package compile another owner's files without declaring that dependency.
The ANT host test has two explicit source seams to substitute UART and locking.
"""
from enum import Enum
from pathlib import Path
import re
import tomllib
from typing import NamedTuple

ROOT = Path(__file__).resolve().parents[1]
OWNERS = {
    "device-api": ("packages/device-api", set()),
    "terrain": ("packages/terrain", set()),
    "firmware-services": ("packages/services", {"device-api"}),
    "firmware-shell": ("packages/shell", {"device-api", "firmware-services"}),
    "firmware-console": ("packages/console", {"device-api", "firmware-services", "firmware-shell"}),
    "vana": ("apps/vana", {"device-api", "firmware-services", "firmware-shell", "terrain"}),
    "c606-firmware": ("devices/c606", {"device-api", "firmware-services", "firmware-shell", "firmware-console", "vana", "terrain"}),
    "shell-simulator": ("tools/simulator", {"device-api", "firmware-services", "firmware-shell", "firmware-console", "vana", "terrain"}),
}
HARDWARE = {"esp_hal", "esp32s3", "esp_radio", "esp_rtos", "esp_storage", "esp_alloc", "esp_println", "esp_backtrace", "esp_bootloader_esp_idf", "c606_firmware", "shell_simulator"}
CONSUMERS = {"firmware_shell", "firmware_console", "vana", "shell_simulator"}
DEVICE_COMPOSITION = {"src/main.rs", "src/commands.rs", "src/logging.rs", "src/trace.rs"}


# The host ANT regression compiles the production owner against fake UART and
# locking. These files are also scanned under their strict implementation rules.
TEST_SOURCE_SEAMS = {
    "devices/c606/tests/ant_power.rs": {
        "../src/drivers/ant_protocol.rs", "../src/capabilities/ant.rs",
    },
}

SINGLE_WRITERS = {
    "Page::EndRide": "apps/vana/src/app/ride_control.rs",
    "Page::DiscardRide": "apps/vana/src/app/ride_control.rs",
    "Page::Summary": "apps/vana/src/app/ride_control.rs",
    "RideMessage::Clear": "apps/vana/src/app/ride_control.rs",
    "RideMessage::StorageNotReady": "apps/vana/src/app/ride_control.rs",
    "Follow::Shown": "apps/vana/src/app/climb_page.rs",
    "Hold::Idle": "apps/vana/src/app/mark.rs",
    "Hold::Holding": "apps/vana/src/app/mark.rs",
    "Hold::Spent": "apps/vana/src/app/mark.rs",
}
ASSIGNMENT = re.compile(r'(?<![=!<>])=\s*(?:\w+\s*::\s*)*(' + '|'.join(
    re.escape(value).replace('::', r'\s*::\s*') for value in SINGLE_WRITERS) + r')\b')

DOMAIN = ('ride', 'kept', 'sensors', 'workout')
LAYERS = ('app', 'screens')
LAYER_RULE = (f'domain code ({", ".join(d + "/" for d in DOMAIN)}) never reaches crate::app or crate::screens: '
              'take what it needs as arguments')
DOMAIN_EXCEPTIONS = {'apps/vana/src/sensors/scan.rs': {('screens', 'sensors', 'MenuMessage')}}
ROOT_ALIAS_RULE = 'domain code may not alias the crate root: spell crate:: paths in full'
TOKEN = re.compile(r'(?:r#)?[A-Za-z_]\w*|::|[^\s$]')

STACK_STATE_WAITER = ('devices/c606/src/drivers/wifi.rs', 'network')
STACK_STATE_WAIT = re.compile(r'(?:\.|::)\s*(wait_(?:config|link)_(?:up|down))\s*\(')
FUNCTION = re.compile(r'\bfn\s+(\w+)')


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


def violations(text, forbidden, portable=False, allowed_paths=()):
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
    return sorted(findings)


def foreign_writes(text, path):
    code = STRING.sub(lambda match: blank(match.group()), without_comments(text))
    findings = []
    for match in ASSIGNMENT.finditer(code):
        value = re.sub(r'\s', '', match[1])
        if path != SINGLE_WRITERS[value]:
            line = code.count('\n', 0, match.start()) + 1
            findings.append((line, f'only {SINGLE_WRITERS[value]} assigns {value}; raise its event instead'))
    return findings


def foreign_stack_waits(text, path):
    code = STRING.sub(lambda match: blank(match.group()), without_comments(text))
    findings = []
    for match in STACK_STATE_WAIT.finditer(code):
        functions = FUNCTION.findall(code, 0, match.start())
        if (path, functions[-1] if functions else None) != STACK_STATE_WAITER:
            line = code.count('\n', 0, match.start()) + 1
            findings.append((line, f'{match[1]} outside `network`: embassy-net wakes one task per stack, and a '
                                   f'second one makes both spin; use config_up or config_down in {STACK_STATE_WAITER[0]} (at most CONFIG_WAITERS of them at once)'))
    return findings


def rust_tokens(text):
    code = STRING.sub(lambda match: blank(match.group()), without_comments(text))
    tokens, line, offset = [], 1, 0
    for match in TOKEN.finditer(code):
        line += code.count('\n', offset, match.start())
        offset = match.start()
        word = match.group()
        tokens.append((word[2:] if word.startswith('r#') and word[2:] not in {'use', 'mod', 'extern', 'as'} else word,
                       line))
    return tokens


def lexical_modules(tokens, module):
    stack, depth, paths = [], 0, []
    for index, (word, _) in enumerate(tokens):
        if word == '{':
            depth += 1
            if index >= 2 and tokens[index - 2][0] == 'mod':
                stack.append((tokens[index - 1][0], depth))
        paths.append(module + tuple(name for name, _ in stack))
        if word == '}':
            if stack and stack[-1][1] == depth:
                stack.pop()
            depth -= 1
    return paths


class Leaf(NamedTuple):
    segments: list
    alias: str | None
    glob: bool
    module: tuple


class PlainFirstSegment(Enum):
    CHILD_MODULE = 'child module'
    UNRESOLVED = 'unresolved'


def use_tree(tokens, index, prefix, module, leaves):
    segments = list(prefix)
    while tokens[index][0] != ';':
        word = tokens[index][0]
        if word == '{':
            index += 1
            while tokens[index][0] not in {'}', ';'}:
                index = use_tree(tokens, index, segments, module, leaves)
                if tokens[index][0] == ',':
                    index += 1
            return index + (tokens[index][0] == '}')
        if word == '*':
            leaves.append(Leaf(segments, None, True, module))
            return index + 1
        segments = segments + [tokens[index]]
        index += 1
        if word == '::' or tokens[index][0] == '::':
            index += word != '::'
            continue
        alias = None
        if tokens[index][0] == 'as':
            alias, index = tokens[index + 1][0], index + 2
        leaves.append(Leaf(segments, alias, False, module))
        return index
    return index


def resolve(segments, module, plain):
    path, line = None, segments[-1][1]
    for position, (name, segment_line) in enumerate(segments):
        if position == 0 and name not in {'crate', 'self', 'super'}:
            if name == '::' or plain is PlainFirstSegment.UNRESOLVED:
                return None
            path = module
        if name == 'crate':
            path = ()
        elif name == 'self':
            path = module if position == 0 else path
        elif name == 'super':
            path = (module if position == 0 else path)[:-1]
        else:
            if not path:
                line = segment_line
            path = path + (name,)
    return path, line


def layer_imports(text, module, allowed=()):
    tokens = rust_tokens(text) + [(';', 0)] * 5
    modules = lexical_modules(tokens, module)
    leaves, expressions, index = [], [], 0
    while index < len(tokens):
        word = tokens[index][0]
        if word == 'use':
            index = use_tree(tokens, index + 1, [], modules[index], leaves)
            continue
        if [w for w, _ in tokens[index:index + 4]] == ['extern', 'crate', 'self', 'as']:
            leaves.append(Leaf([tokens[index + 1]], tokens[index + 4][0], False, modules[index]))
            index += 5
            continue
        if re.match(r'\w', word) and tokens[index + 1][0] == '::' and (index == 0 or tokens[index - 1][0] != '::'):
            end = index
            while re.match(r'\w', tokens[end][0]) and tokens[end + 1][0] == '::':
                end += 2
            expressions.append(([token for token in tokens[index:end + 1] if token[0] != '::'], modules[index]))
            index = end + 1
            continue
        index += 1

    findings = []
    paths = [(segments, alias, glob, scope, PlainFirstSegment.CHILD_MODULE) for segments, alias, glob, scope in leaves]
    paths += [(segments, None, False, scope, PlainFirstSegment.UNRESOLVED) for segments, scope in expressions]
    for segments, alias, glob, scope, plain in paths:
        named = resolve(segments, scope, plain)
        if named is None:
            continue
        path, line = named
        if alias and path == ():
            findings.append((line, f'`as {alias}`: {ROOT_ALIAS_RULE}'))
        elif glob and path == ():
            findings.append((line, f'a glob of the crate root brings crate::app and crate::screens in; {LAYER_RULE}'))
        elif path and path[0] in LAYERS and path not in allowed:
            findings.append((line, f'crate::{"::".join(path)}: {LAYER_RULE}'))
    return sorted(set(findings))


def layer_findings(text, repo_relative_path):
    package = Path('apps/vana')
    if not Path(repo_relative_path).is_relative_to(package):
        return []
    module = domain_module(Path(repo_relative_path).relative_to(package))
    return layer_imports(text, module, DOMAIN_EXCEPTIONS.get(repo_relative_path, ())) if module else []


def domain_module(relative):
    parts = relative.with_suffix('').parts
    if len(parts) < 2 or parts[0] != 'src' or parts[1] not in DOMAIN:
        return None
    return parts[1:-1] if parts[-1] == 'mod' else parts[1:]


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
        forbidden = set() if composition else CONSUMERS | {'main', 'commands', 'logging', 'trace'}
        portable = False
    elif owner == 'shell-simulator':
        forbidden, portable = {'c606_firmware'}, False
    else:
        allowed = OWNERS[owner][1]
        forbidden = HARDWARE | {name.replace('-', '_') for name in OWNERS
                                if name != owner and name not in allowed}
        portable = True
    forbidden |= {alias.replace('-', '_') for alias, package in aliases
                  if package.replace('-', '_') in forbidden}
    return forbidden, portable


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
    elsewhere = 'apps/vana/src/app/input.rs'
    for source in ['self.navigation.page = Page::EndRide;', 'page = crate::screens::workout::Page::Summary;',
                   'navigation.page=Page :: EndRide']:
        assert foreign_writes(source, elsewhere), source
    for source in ['if page == Page::EndRide {}', 'page != Page::Summary', '// page = Page::EndRide',
                   'let name = "page = Page::EndRide";', 'page = Page::EndRideLater;']:
        assert not foreign_writes(source, elsewhere), source
    assert not foreign_writes('self.navigation.page = Page::EndRide;', SINGLE_WRITERS['Page::EndRide'])
    recorder, deeper = ('ride', 'recorder'), ('ride', 'recorder', 'invariants')
    for source in ['use crate::app::Runtime;', 'let page = crate :: screens::workout::Page::Home;',
                   'use crate::{ride::log, app::Medium};', 'use crate::{\n    screens::{sensors},\n};',
                   'use super::super::app::Medium;', 'use super::super::{log, app};',
                   'use super::{super::app::Runtime};', 'pub use crate::{{app::Runtime}};',
                   'pub use crate::r#app::Runtime;', 'pub(crate) use crate::screens;', 'use crate::screens::{self};',
                   'use crate::*;', 'use super::super::*;',
                   'fn f() { crate::app::x(); }', 'fn f() { super::super::app::x(); }',
                   'fn f() { <crate::app::Runtime as Default>::default(); }', 'fn f() { crate::r#screens::X; }',
                   'mod inner { use super::super::super::app::Local; }']:
        assert layer_imports(source, recorder), source
    for source in ['use crate as root; pub use root::app::Runtime;', 'pub use root::app::Runtime; use crate as root;',
                   'use crate::{self as root}; use root::screens::Shown;', 'use super::super as root; use root::app;',
                   'extern crate self as root; fn f() { root::app::x(); }', 'use crate::{self as root};',
                   'use crate as root; use self::root::app::X;', 'use crate as root; mod inner { use super::root::app::X; }',
                   'use super::{super as root};', 'pub(crate) use crate as root;',
                   'use crate as root; mod inner { use super::root as x; fn f() { x::app::Runtime; } }']:
        assert any(ROOT_ALIAS_RULE in reason for _, reason in layer_imports(source, recorder)), source
    local_alias = 'fn f() {\n    use crate as root;\n}\nmod root { pub mod app { pub fn local() {} } }\nfn g() { root::app::local(); }'
    assert layer_imports(local_alias, recorder) == [(2, f'`as root`: {ROOT_ALIAS_RULE}')]
    shadowed = 'fn f() { use crate as x; }\nmod x { pub mod app { pub struct Runtime; } }\nfn g() { let _ = x::app::Runtime; }'
    assert layer_imports(shadowed, recorder) == [(1, f'`as x`: {ROOT_ALIAS_RULE}')]
    for source in ['use super::super::{super::app};', 'use super::super::super::app::Runtime;']:
        assert layer_imports(source, deeper), source
    assert layer_imports('use super::*;', ('ride',))
    assert layer_imports('use super::app::X;', ('ride',))
    for source in ['use crate::ride::log::Status;', 'use super::journal::Journal;', 'use super::app::Local;',
                   'use crate::{ride::{app}, sensors};', '// use crate::app::Runtime;', '/* use crate::app::X; */',
                   'let doc = "crate::app";', 'use crate::application::X;', 'fn f() { crate::application::x(); }',
                   'use super::*;', 'use super as ride;', 'use crate::ride as root;', 'use self::app::Local;', 'use app::Local;', 'use device_api::app::X;', 'self.app.x();',
                   'fn f() -> impl Iterator<Item = u8> + use<> { todo!() }',
                   'mod app { pub struct Local; } mod inner { mod deeper { use super::super::app::Local; } }']:
        assert not layer_imports(source, recorder), source
    assert [line for line, _ in layer_imports('use crate::{\n    ride::log,\n    app::\n        Medium,\n};', recorder)] == [3]
    scan = 'apps/vana/src/sensors/scan.rs'
    menu = 'use crate::screens::sensors::MenuMessage;\n'
    assert not layer_findings(menu, scan)
    assert [line for line, _ in layer_findings(menu + 'use crate::app::Runtime;', scan)] == [2]
    assert [line for line, _ in layer_findings(menu, 'apps/vana/src/sensors/saved.rs')] == [1]
    assert not layer_findings(menu, 'apps/vana/src/app/input.rs')
    assert domain_module(Path('src/ride/recorder.rs')) == recorder
    assert domain_module(Path('src/sensors/mod.rs')) == ('sensors',)
    assert domain_module(Path('src/kept/facts/store.rs')) == ('kept', 'facts', 'store')
    assert [line for line, _ in layer_findings('use crate::app::Runtime;', 'apps/vana/src/kept/bests.rs')] == [1]
    assert domain_module(Path('src/app/input.rs')) is None
    assert domain_module(Path('tests/ride.rs')) is None
    wifi = STACK_STATE_WAITER[0]
    publisher = 'async fn network(stack: Stack) {\n    loop {\n        stack.wait_config_up().await;\n' \
                '        stack\n            .wait_config_down()\n            .await;\n    }\n}\n'
    assert not foreign_stack_waits(publisher, wifi)
    assert not foreign_stack_waits('// fn f() { stack.wait_link_up() }\nlet doc = "stack.wait_config_up()";', wifi)
    assert [line for line, _ in foreign_stack_waits(publisher, 'devices/c606/src/drivers/http.rs')] == [3, 5]
    for helper in ['async fn up(stack: Stack) { stack.wait_config_up().await }',
                   'fn verify() { Stack::wait_config_down(&stack).await; }',
                   'fn time_sync() { select(stack.wait_link_down(), timer).await; }']:
        assert foreign_stack_waits(publisher + helper, wifi), helper


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
            failures.extend(f'{path.relative_to(ROOT)}:{line}: {reason}'
                            for line, reason in foreign_writes(path.read_text(), path.relative_to(ROOT).as_posix()))
            failures.extend(f'{path.relative_to(ROOT)}:{line}: {reason}'
                            for line, reason in layer_findings(path.read_text(), path.relative_to(ROOT).as_posix()))
            failures.extend(f'{path.relative_to(ROOT)}:{line}: {reason}'
                            for line, reason in foreign_stack_waits(path.read_text(), path.relative_to(ROOT).as_posix()))
    if failures:
        raise SystemExit('\n'.join(failures))
    print(f'Boundaries, layers, single writers and the stack waiter passed for {len(OWNERS)} packages and {len(paths)} Rust files.')


if __name__ == '__main__':
    main()
