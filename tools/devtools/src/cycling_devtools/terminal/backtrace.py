"""Decode code addresses in device logs into functions, files and lines.

    cycling backtrace [log ...] [--elf PATH]     # stdin when no log is given
    cycling device terminal CRASH | cycling backtrace

A panic prints its message and `0x42xxxxxx` return addresses on the USB
console, and keeps them in the device's crash record, which `CRASH` reads
back after the reset. Each line that holds code addresses is followed by what
they are in the ELF the image came from. build.sh keeps the newest builds'
ELFs in the primary checkout as .local/elf/<commit>[-dirty]-<variant>.elf and
the last one as .local/cycling.elf. An `elf=` in the input (a CRASH reply)
picks its own; a `commit=` must match the ELF used, or decoding stops rather
than name the wrong functions. Decode on the machine that built the image.
"""

import argparse
import os
import re
import subprocess
import sys
from pathlib import Path

from cycling_devtools.workspace import DEVICE_LOCAL, ROOT

# ESP32-S3 instruction memory: internal IRAM 0x4037_0000.. and flash-mapped
# code 0x4200_0000..0x4400_0000. Data addresses never fall here.
CODE = re.compile(r'0x(4[0-3][0-9a-fA-F]{6})\b')


def addr2line():
    toolchain = os.environ.get('CYCLING_ESP_TOOLCHAIN')
    if not toolchain:
        raise RuntimeError('Enter the dev shell first: the decoder comes with the Xtensa toolchain')
    return Path(toolchain) / 'xtensa-esp-elf/bin/xtensa-esp32s3-elf-addr2line'


def is_code(address):
    value = int(address, 16)
    return 0x4037_0000 <= value < 0x403E_0000 or 0x4200_0000 <= value < 0x4400_0000


def resolve(elf, addresses):
    """One addr2line run for every address; inlined frames stay together."""
    if not addresses:
        return {}
    out = subprocess.run(
        [str(addr2line()), '-e', str(elf), '-a', '-f', '-i', '-C', '-p', *addresses],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    frames, current = {}, None
    for line in out.splitlines():
        match = re.match(r'0x0*([0-9a-f]+): (.*)', line)
        if match:
            current = f'0x{match.group(1)}'
            frames[current] = [match.group(2)]
        elif current and line.strip().startswith('(inlined by)'):
            frames[current].append(line.strip())
    return frames


def decode(lines, elf):
    found = []
    for line in lines:
        found += [f'0x{a.lower()}' for a in CODE.findall(line) if is_code(a) and f'0x{a.lower()}' not in found]
    frames = resolve(elf, found)
    for line in lines:
        yield line.rstrip('\n')
        for address in dict.fromkeys(f'0x{a.lower()}' for a in CODE.findall(line)):
            for frame in frames.get(address, []):
                if '??' not in frame:
                    yield f'    {address} {frame}' if not frame.startswith('(') else f'        {frame}'


COMMIT = re.compile(r'\bcommit=([0-9a-f]{7,40})\b')
ELF = re.compile(r'\belf=([0-9a-f]{7,40}(?:-dirty)?-sdk[01]-h[01](?:-maint)?)\b')


def choose_elf(lines, archive):
    """The ELF the input names, else the last build's; and the commit it must hold."""
    text = ''.join(lines)
    commit = COMMIT.search(text)
    named = ELF.search(text)
    if named and (archive / f'{named.group(1)}.elf').is_file():
        return archive / f'{named.group(1)}.elf', commit and commit.group(1)
    if named:
        print(f'no ELF kept as {named.group(1)}; trying the last build', file=sys.stderr)
    return ROOT / '.local/cycling.elf', commit and commit.group(1)


def holds_commit(elf, commit):
    """Firmware embeds its build commit as text, so the right ELF contains it."""
    return commit.encode() in elf.read_bytes()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('logs', nargs='*', type=Path)
    parser.add_argument(
        '--elf', type=Path, help='default: the ELF of the commit= in the input, else .local/cycling.elf'
    )
    args = parser.parse_args(argv)
    lines = [line for log in args.logs for line in log.read_text(errors='replace').splitlines(True)] or list(sys.stdin)
    if args.elf:
        elf, commit = args.elf, None
    else:
        elf, commit = choose_elf(lines, DEVICE_LOCAL / 'elf')
    if not elf.is_file():
        sys.exit(f'{elf} is missing: build the image that crashed first (cycling build)')
    if commit and not holds_commit(elf, commit):
        sys.exit(f'{elf} was not built from commit {commit}: build that commit, or pass --elf')
    for line in decode(lines, elf):
        print(line)


if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
