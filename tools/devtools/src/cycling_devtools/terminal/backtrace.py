"""Decode code addresses in device logs into functions, files and lines.

    mise run backtrace [log ...] [--elf PATH]     # stdin when no log is given
    mise run device terminal CRASH | mise run backtrace

A panic prints its message and `0x42xxxxxx` return addresses on the USB
console, and keeps them in the device's crash record, which `CRASH` reads
back after the reset. Each line that holds code addresses is followed by what
they are in the ELF the image came from. build.sh keeps every build's ELF as
.local/elf/<commit>.elf and the last one as .local/cycling.elf; a `commit=`
in the input (a CRASH reply) picks its own, so decode on the machine that
built the image that crashed.
"""

import argparse
import os
import re
import subprocess
import sys
from pathlib import Path

from cycling_devtools.workspace import ROOT

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


def choose_elf(lines, archive):
    """The ELF built from the commit the input names, else the last build's."""
    for line in lines:
        match = COMMIT.search(line)
        if match:
            commit = match.group(1)
            for name in (f'{commit}.elf', f'{commit}-dirty.elf'):
                if (archive / name).is_file():
                    return archive / name
            print(f'no ELF kept for commit {commit}; decoding with the last build', file=sys.stderr)
            break
    return ROOT / '.local/cycling.elf'


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('logs', nargs='*', type=Path)
    parser.add_argument('--elf', type=Path, help='default: the ELF of the commit= in the input, else .local/cycling.elf')
    args = parser.parse_args(argv)
    lines = [line for log in args.logs for line in log.read_text(errors='replace').splitlines(True)] or list(sys.stdin)
    elf = args.elf or choose_elf(lines, ROOT / '.local/elf')
    if not elf.is_file():
        sys.exit(f'{elf} is missing: build the image that crashed first (mise run build)')
    for line in decode(lines, elf):
        print(line)


if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
