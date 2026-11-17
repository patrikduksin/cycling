"""Name a build by its content, archive what it was built from in the primary checkout's .local/builds, and relate
archived builds."""

import argparse
import hashlib
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import NamedTuple

from cycling_devtools import tree, workspace

HEADER_BUILD_BYTES = 20
BUILD_SOURCES = Path('tools') / 'simulator' / 'build-sources'


class Archived(NamedTuple):
    sources: dict[str, str]
    vana_features: frozenset[str]


def cargo_feature_names(features):
    """Cargo's CARGO_FEATURE_<name> spelling, lowercased, so build.sh's requested list and the build scripts'
    CARGO_FEATURE_ variables name a feature alike."""
    return sorted({name.strip().lower().replace('-', '_') for name in features} - {''})


def sources(root, treeish):
    paths = (root / BUILD_SOURCES).read_text().split()
    ids = {}
    for line in tree.git('ls-tree', treeish, '--', *paths, cwd=root).splitlines():
        entry, path = line.split('\t')
        ids[path] = entry.split()[2]
    return {path: ids[path] for path in paths if path in ids}


def listing(root, rustc, scope, features, vana_features):
    built = tree.tree_hash(root)
    toolchain = subprocess.run([rustc, '-vV'], check=True, capture_output=True, text=True).stdout
    lines = [f'tree {built}'] if scope == 'tree' else []
    lines += [f'source {path} {object_id}' for path, object_id in sources(root, built).items()]
    lines += [
        f'features {",".join(cargo_feature_names(features))}',
        f'vana-features {",".join(cargo_feature_names(vana_features))}',
    ]
    lines += [f'toolchain {line}' for line in toolchain.splitlines()]
    return ''.join(f'{line}\n' for line in lines), built


def digest(text):
    return hashlib.sha256(text.encode()).hexdigest()[: 2 * HEADER_BUILD_BYTES]


def builds_dir(root):
    return workspace.main_checkout(root) / '.local' / 'builds'


def publish_once(scratch, archived):
    try:
        scratch.rename(archived)
    except OSError:
        if not archived.is_dir():
            raise


def archive(root, name, text, built, dirty):
    builds = builds_dir(root)
    if (builds / name).is_dir():
        return
    commit = tree.git('rev-parse', 'HEAD', cwd=root)
    builds.mkdir(parents=True, exist_ok=True)
    scratch = Path(tempfile.mkdtemp(dir=builds, prefix=f'.{name}.'))
    try:
        (scratch / 'listing').write_text(text)
        (scratch / 'commit').write_text(f'{commit}\n')
        if dirty:
            patch = subprocess.run(
                ['git', 'diff', '--binary', commit, built], cwd=root, check=True, capture_output=True
            ).stdout
            (scratch / 'diff.patch').write_bytes(patch)
        publish_once(scratch, builds / name)
    finally:
        shutil.rmtree(scratch, ignore_errors=True)


def read(builds, name) -> Archived | None:
    try:
        text = (builds / name / 'listing').read_text()
    except OSError:
        return None
    found, vana = {}, frozenset()
    for line in text.splitlines():
        key, _, value = line.partition(' ')
        if key == 'source':
            path, object_id = value.split(' ')
            found[path] = object_id
        elif key == 'vana-features':
            vana = frozenset(value.split(',')) - {''}
    return Archived(found, vana) if found else None


def relate(builds, a, b):
    if a == b:
        return 'same'
    archived = {name: read(builds, name) for name in (a, b)}
    for name, found in archived.items():
        if found is None:
            return f'unknown build {name[:12]} has no archived sources on this machine'
    return 'same-sources' if archived[a] == archived[b] else 'other'


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=workspace.ROOT)
    commands = parser.add_subparsers(dest='command', required=True)
    named = commands.add_parser('hash', help='print the hash of this tree and whether it differs from HEAD')
    named.add_argument('--scope', choices=['tree', 'sources'], required=True)
    named.add_argument('--rustc', default=os.environ.get('RUSTC', 'rustc'))
    named.add_argument('--features', default='', help='comma-separated, as cargo was given them')
    named.add_argument('--vana-features', default='', help='comma-separated, as vana was built with them')
    related = commands.add_parser('relate', help='print how two builds relate')
    related.add_argument('a')
    related.add_argument('b')
    built_from = commands.add_parser('built-from', help="exit 0 when the build's sources are the revision's")
    built_from.add_argument('build')
    built_from.add_argument('revision')
    shown = commands.add_parser('listing', help="print a build's archived listing")
    shown.add_argument('build')
    args = parser.parse_args(argv)
    match args.command:
        case 'hash':
            text, built = listing(
                args.root, args.rustc, args.scope, args.features.split(','), args.vana_features.split(',')
            )
            name = digest(text)
            dirty = built != tree.git('rev-parse', 'HEAD^{tree}', cwd=args.root)
            archive(args.root, name, text, built, dirty)
            print(name, 'dirty' if dirty else 'clean')
            return 0
        case 'relate':
            print(relate(builds_dir(args.root), args.a, args.b))
            return 0
        case 'built-from':
            found = read(builds_dir(args.root), args.build)
            return 0 if found is not None and found.sources == sources(args.root, args.revision) else 1
        case 'listing':
            archived = builds_dir(args.root) / args.build
            try:
                print((archived / 'listing').read_text(), end='')
            except OSError:
                print(f'build {args.build} is not archived on this machine', file=sys.stderr)
                return 1
            return 0
    raise AssertionError(args.command)


if __name__ == '__main__':
    sys.exit(main())
