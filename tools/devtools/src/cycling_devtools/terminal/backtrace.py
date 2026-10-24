"""Decode code addresses in device logs into functions, files and lines.

    mise run backtrace [log ...] [--elf PATH]     # stdin when no log is given

esp-backtrace prints the panic message and `0x42xxxxxx` return addresses on
the USB console. Each line that holds code addresses is followed by what they
are in the ELF the image came from: build.sh keeps it as .local/cycling.elf,
so decode on the machine that built the image that crashed.
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


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('logs', nargs='*', type=Path)
    parser.add_argument('--elf', type=Path, default=ROOT / '.local/cycling.elf')
    args = parser.parse_args(argv)
    if not args.elf.is_file():
        sys.exit(f'{args.elf} is missing: build the image that crashed first (mise run build)')
    lines = [line for log in args.logs for line in log.read_text(errors='replace').splitlines(True)] or list(sys.stdin)
    for line in decode(lines, args.elf):
        print(line)


if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
