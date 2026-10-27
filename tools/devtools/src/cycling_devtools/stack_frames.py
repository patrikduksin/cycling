"""Check each firmware function's stack frame against its budget in devices/c606/stack-frames.toml.

Every windowed-ABI Xtensa function opens with `entry a1, N`, which moves the
stack pointer down by its whole N-byte frame. LLVM caps that immediate at
32,760 B; a larger frame opens with `entry a1, 32` and subtracts the rest
from a literal before `movsp a1`. The budgets bound single frames only: a
deep chain of small frames can still exhaust the stack, which only STATUS
stack_untouched on the device measures.
"""

import argparse
import os
import re
import shutil
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path

from cycling_devtools.workspace import ROOT

BUDGETS = ROOT / 'devices' / 'c606' / 'stack-frames.toml'
# Inlining shifts frames after unrelated edits, by about 300 B on the 16-20 KB
# frames, while the frames worth catching (a page canvas, a boot View) are
# kilobytes.
MIN_MARGIN = 1024
LARGE_FRAME_ENTRY = 32
CATCH_ALL = '*'

LABEL = re.compile(r'^[0-9a-f]+ <(?P<name>.+)>:$')
INSTRUCTION = re.compile(r'^\s*[0-9a-f]+:\s+(?P<op>\S+)\s*(?P<args>.*)$')
LITERAL = re.compile(r'\((?P<value>[0-9a-f]+) <')


@dataclass(frozen=True)
class Rule:
    pattern: str
    budget: int

    def matches(self, name):
        return re.fullmatch('.*'.join(map(re.escape, self.pattern.split('*'))), name) is not None


def margin(size):
    return max(MIN_MARGIN, -(-size // 10))


@dataclass(frozen=True)
class Frame:
    name: str
    size: int
    rule: Rule | None

    @property
    def over(self):
        return self.rule is not None and self.size > self.rule.budget


def frame_size(name, prologue):
    """The frame its first instructions allocate, or None for code without a windowed entry."""
    if not prologue or prologue[0][0] != 'entry':
        return None
    register, size = (part.strip() for part in prologue[0][1].split(','))
    if register != 'a1':
        return None
    size = int(size, 0)
    rest = prologue[1:4]
    subtracts = any(op == 'sub' and ', a1, ' in args for op, args in rest)
    moves = any(op == 'movsp' and args.startswith('a1,') for op, args in rest)
    if not subtracts and not (size == LARGE_FRAME_ENTRY and moves):
        return size
    literal = LITERAL.search(rest[0][1])
    if size != LARGE_FRAME_ENTRY or [op for op, _ in rest] != ['l32r', 'sub', 'movsp'] or literal is None:
        raise ValueError(f'unrecognised large-frame prologue in {name}: {prologue}')
    return size + int(literal['value'], 16)


def parse(disassembly):
    """Frame size per demangled function name from `objdump -d -C` output."""
    prologues = []
    for line in disassembly.splitlines():
        if label := LABEL.match(line):
            prologues.append((label['name'], []))
        elif prologues and len(prologues[-1][1]) < 4 and (instruction := INSTRUCTION.match(line)):
            prologues[-1][1].append((instruction['op'], instruction['args']))
    sizes = {}
    for name, prologue in prologues:
        size = frame_size(name, prologue)
        if size is not None:
            sizes[name] = max(size, sizes.get(name, 0))
    return sizes


def load_rules(path, variant):
    """The variant's rules in file order, or None when the file has no table for it."""
    table = tomllib.loads(Path(path).read_text()).get(variant)
    if table is None:
        return None
    return [Rule(pattern, budget) for pattern, budget in table.items()]


def assess(sizes, rules):
    frames = [
        Frame(name, size, next((rule for rule in rules if rule.matches(name)), None)) for name, size in sizes.items()
    ]
    return sorted(frames, key=lambda frame: (-frame.size, frame.name))


def unused(frames, rules):
    used = {frame.rule for frame in frames}
    return [rule for rule in rules if rule not in used]


def warnings(frames, rules, budgets, variant):
    return [
        f'WARNING: stack frame rule "{rule.pattern}" in {budgets.name} [{variant}] matches no function; '
        'update or delete it.'
        for rule in unused(frames, rules)
    ]


def report(frames, variant, top, notes=()):
    rows = [
        f'#### Largest stack frames, {variant}',
        '',
        '| frame | bytes | budget | rule |',
        '| --- | ---: | ---: | --- |',
    ]
    for frame in frames[:top]:
        budget = f'{frame.rule.budget:,}' if frame.rule else 'none'
        rule = f'`{frame.rule.pattern}`' if frame.rule and frame.rule.pattern != frame.name else ''
        mark = ' ❌' if frame.over else ''
        rows.append(f'| `{frame.name}` | {frame.size:,}{mark} | {budget} | {rule} |')
    if notes:
        rows += ['', *notes]
    return '\n'.join(rows) + '\n'


def suggest(frames, rules, variant):
    """The variant's table with each named budget reset to its largest frame plus its margin.

    The catch-all keeps its budget: a frame over it gets its own exact rule, so
    no unnamed frame can grow unseen. Rules that match nothing are dropped.
    """
    largest = {}
    for frame in frames:
        if frame.rule is not None:
            largest[frame.rule] = max(largest.get(frame.rule, 0), frame.size)
    lines = [f'[{variant}]']
    for rule in rules:
        if rule.pattern == CATCH_ALL:
            lines += [f'"{f.name}" = {f.size + margin(f.size)}' for f in frames if f.rule == rule and f.over]
            lines.append(f'"{rule.pattern}" = {rule.budget}')
        elif rule in largest:
            lines.append(f'"{rule.pattern}" = {largest[rule] + margin(largest[rule])}')
    return '\n'.join(lines) + '\n'


def default_objdump():
    toolchain = os.environ.get('CYCLING_ESP_TOOLCHAIN')
    name = 'xtensa-esp32s3-elf-objdump'
    return str(Path(toolchain) / 'xtensa-esp-elf' / 'bin' / name) if toolchain else name


def disassemble(elf, objdump):
    return subprocess.run(
        [objdump, '-d', '-C', '--no-show-raw-insn', str(elf)], capture_output=True, text=True, check=True
    ).stdout


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('elf', type=Path)
    parser.add_argument('--variant', required=True, help='budget table to apply, such as sdk1-h1')
    parser.add_argument('--budgets', type=Path, default=BUDGETS)
    parser.add_argument('--objdump', default=default_objdump())
    parser.add_argument('--top', type=int, default=10)
    parser.add_argument('--report', type=Path, help='also write the top-frame table here')
    parser.add_argument(
        '--suggest', action='store_true', help='print the table with budgets at current + max(1 KiB, 10%%)'
    )
    args = parser.parse_args(argv)

    if shutil.which(args.objdump) is None:
        parser.error(f'no xtensa objdump at {args.objdump}; run inside the dev shell (nix develop) or pass --objdump')
    if not args.elf.is_file():
        parser.error(f'no ELF at {args.elf}')
    sizes = parse(disassemble(args.elf, args.objdump))
    rules = load_rules(args.budgets, args.variant)
    if rules is None:
        print(f'{args.budgets.name} has no [{args.variant}] table; frame sizes are not checked.', file=sys.stderr)
        rules = []
    frames = assess(sizes, rules)
    notes = warnings(frames, rules, args.budgets, args.variant)
    if args.suggest:
        for note in notes:
            print(f'Dropped: {note.removeprefix("WARNING: ")}', file=sys.stderr)
        print(suggest(frames, rules, args.variant), end='')
        return 0

    table = report(frames, args.variant, args.top, notes)
    print(table, end='', flush=True)
    if args.report:
        args.report.write_text(table)
    failures = [
        f'Stack frame over budget: {frame.name} is {frame.size:,} B, budget {frame.rule.budget:,} B '
        f'(rule "{frame.rule.pattern}" in {args.budgets.name} [{args.variant}]).'
        for frame in frames
        if frame.over
    ]
    for failure in failures:
        print(failure, file=sys.stderr)
    if failures:
        print(
            'Keep big temporaries out of stack frames, as 40aa736 (#205) did. For a deliberate change, '
            "print this build's table with each budget at its frame + max(1 KiB, 10%):\n"
            f'  cycling stack-frames {args.elf.resolve()} --variant {args.variant} --suggest',
            file=sys.stderr,
        )
    return 1 if failures else 0


if __name__ == '__main__':
    sys.exit(main())
