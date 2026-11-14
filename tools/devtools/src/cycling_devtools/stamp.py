"""Write a build's commit into a linked firmware ELF over the placeholder it was compiled with."""

import argparse
import sys
from pathlib import Path


def stamp(elf, placeholder, commit):
    if len(placeholder) != len(commit):
        raise ValueError(f'placeholder {placeholder!r} and commit {commit!r} differ in length')
    if placeholder not in elf:
        raise ValueError(
            f'placeholder {placeholder.decode()!r} is not in the ELF: the firmware no longer embeds '
            'CYCLING_BUILD_COMMIT as a string, so stamping cannot name its commit'
        )
    return elf.replace(placeholder, commit)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('elf', type=Path)
    parser.add_argument('out', type=Path)
    parser.add_argument('--placeholder', required=True)
    parser.add_argument('--commit', required=True)
    args = parser.parse_args(argv)
    try:
        stamped = stamp(args.elf.read_bytes(), args.placeholder.encode(), args.commit.encode())
    except ValueError as error:
        print(error, file=sys.stderr)
        return 1
    args.out.write_bytes(stamped)
    return 0


if __name__ == '__main__':
    sys.exit(main())
