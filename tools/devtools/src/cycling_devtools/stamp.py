"""Write a build's commit and hash into a linked firmware ELF over the placeholders it was compiled with."""

import argparse
import sys
from pathlib import Path


def stamp(elf, placeholder, value):
    if len(placeholder) != len(value):
        raise ValueError(f'placeholder {placeholder!r} and value {value!r} differ in length')
    if placeholder not in elf:
        raise ValueError(
            f'placeholder {placeholder.decode()!r} is not in the ELF: the firmware no longer embeds '
            'it as a string, so stamping cannot name its build'
        )
    return elf.replace(placeholder, value)


def pair(text):
    placeholder, equals, value = text.partition('=')
    if not equals:
        raise argparse.ArgumentTypeError(f'{text!r} is not PLACEHOLDER=VALUE')
    return placeholder.encode(), value.encode()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('elf', type=Path)
    parser.add_argument('out', type=Path)
    parser.add_argument('--stamp', action='append', required=True, type=pair, metavar='PLACEHOLDER=VALUE')
    args = parser.parse_args(argv)
    stamped = args.elf.read_bytes()
    try:
        for placeholder, value in args.stamp:
            stamped = stamp(stamped, placeholder, value)
    except ValueError as error:
        print(error, file=sys.stderr)
        return 1
    args.out.write_bytes(stamped)
    return 0


if __name__ == '__main__':
    sys.exit(main())
