"""Free disk: remove merged worktrees and idle build directories (a dry run unless --apply).

Every linked worktree carries its own .local/ build targets, about 4.5 GB each, and
agents leave them behind. A worktree is removed only when it is provably dead: its
branch is merged, its tree is clean and no process works inside it. Build
directories of the worktrees that stay are deleted when nothing in them changed for
--idle-hours. The primary checkout's sources and its .local/elf archive are never touched.
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from pathlib import Path

from cycling_devtools.workspace import ROOT

# Under <worktree>/.local: regenerable by build.sh, simulate or verify.
BUILD_DIR_PATTERNS = ('firmware*', 'sim', 'verify')
IDLE_HOURS = 24.0
LOW_DISK_GB = 30
GB = 10**9


@dataclass(frozen=True)
class Worktree:
    path: Path
    head: str
    branch: str | None
    primary: bool
    locked: bool
    missing: bool


@dataclass(frozen=True)
class Decision:
    action: str  # 'remove' or 'keep'
    reason: str
    branch_force: bool = False  # a squash-merged branch is not an ancestor, so -D


@dataclass(frozen=True)
class Row:
    kind: str  # 'worktree' or 'build dir'
    path: Path
    size: int
    action: str  # 'remove' or 'keep'
    reason: str
    branch: str | None = None
    branch_force: bool = False


@dataclass
class Plan:
    rows: list[Row] = field(default_factory=list)

    @property
    def removable(self):
        return [row for row in self.rows if row.action == 'remove']


def run(*command, cwd=ROOT):
    return subprocess.run(command, cwd=cwd, capture_output=True, text=True, timeout=120)


def worktrees(root=ROOT):
    """Parse `git worktree list --porcelain`; the first entry is the primary checkout."""
    out = run('git', 'worktree', 'list', '--porcelain', cwd=root)
    if out.returncode:
        raise RuntimeError(out.stderr.strip())
    found = []
    for block in out.stdout.strip().split('\n\n'):
        fields = {key: value for key, _, value in (line.partition(' ') for line in block.splitlines())}
        path = Path(fields['worktree'])
        branch = fields.get('branch')
        found.append(
            Worktree(
                path=path.resolve(),
                head=fields.get('HEAD', ''),
                branch=branch.removeprefix('refs/heads/') if branch else None,
                primary=not found,
                locked='locked' in fields,
                missing=not path.is_dir(),
            )
        )
    return found


def live_processes(trees, proc=Path('/proc')):
    """Map each worktree to the (pid, command) of processes whose cwd is inside it.

    A cwd belongs to the deepest worktree containing it, because agent worktrees are
    nested inside the primary checkout. Fails closed when /proc cannot be read.
    """
    if not (proc / str(os.getpid()) / 'cwd').exists():
        raise RuntimeError(f'cannot read {proc}/<pid>/cwd, so live agents cannot be detected')
    live = {tree.path: [] for tree in trees}
    for entry in proc.iterdir():
        if not entry.name.isdigit():
            continue
        try:
            cwd = Path(os.readlink(entry / 'cwd').removesuffix(' (deleted)'))
            command = (entry / 'cmdline').read_bytes().replace(b'\0', b' ').decode(errors='replace').strip()
        except OSError:
            continue
        owners = [tree for tree in trees if tree.path == cwd or tree.path in cwd.parents]
        if owners:
            deepest = max(owners, key=lambda tree: len(tree.path.parts))
            live[deepest.path].append((int(entry.name), command[:60]))
    return live


def dirty(path):
    """Uncommitted or untracked changes outside ignored paths; unknown counts as dirty."""
    out = run('git', 'status', '--porcelain', '--untracked-files=all', cwd=path)
    return out.returncode != 0 or bool(out.stdout.strip())


def in_main(head, root=ROOT):
    return run('git', 'merge-base', '--is-ancestor', head, 'origin/main', cwd=root).returncode == 0


def merged_pull_request(branch, head, root=ROOT):
    """The number of a merged PR whose tip is HEAD or contains it, else None.

    Matching the tip, not just the branch name, keeps commits pushed to a reused
    branch name after the merge from being treated as merged.
    """
    out = run(
        'gh', 'pr', 'list', '--state', 'merged', '--head', branch,
        '--json', 'number,headRefOid', '--limit', '10',
        cwd=root,
    )  # fmt: skip
    if out.returncode:
        return None
    for pull in json.loads(out.stdout or '[]'):
        tip = pull['headRefOid']
        if tip == head or run('git', 'merge-base', '--is-ancestor', head, tip, cwd=root).returncode == 0:
            return pull['number']
    return None


def judge(tree, current, live, root=ROOT):
    """Why a worktree stays, or the proof that it is dead. The first failing rule wins."""
    if tree.primary:
        return Decision('keep', 'primary checkout')
    if tree.path == current:
        return Decision('keep', 'current worktree')
    if live[tree.path]:
        pid, command = live[tree.path][0]
        return Decision('keep', f'live process {pid} ({command})')
    if tree.locked:
        return Decision('keep', 'locked')
    if tree.missing:
        return Decision('keep', 'directory missing (git worktree prune)')
    if dirty(tree.path):
        return Decision('keep', 'uncommitted or untracked changes')
    if in_main(tree.head, root):
        return Decision('remove', 'HEAD is in origin/main, tree clean, no live process')
    if tree.branch and (number := merged_pull_request(tree.branch, tree.head, root)):
        return Decision('remove', f'PR #{number} merged, tree clean, no live process', branch_force=True)
    return Decision('keep', 'not merged')


def measure(path):
    """(disk bytes, newest mtime) of a tree without following links; hard links count once."""
    try:
        newest = path.stat().st_mtime
    except OSError:
        return 0, 0.0
    seen = set()
    size = 0
    stack = [path]
    while stack:
        try:
            with os.scandir(stack.pop()) as entries:
                for entry in entries:
                    info = entry.stat(follow_symlinks=False)
                    newest = max(newest, info.st_mtime)
                    if entry.is_dir(follow_symlinks=False):
                        stack.append(entry.path)
                    if (info.st_dev, info.st_ino) not in seen:
                        seen.add((info.st_dev, info.st_ino))
                        size += info.st_blocks * 512
        except OSError:
            continue
    return size, newest


def build_dirs(tree):
    local = tree.path / '.local'
    found = {child for pattern in BUILD_DIR_PATTERNS for child in local.glob(pattern)}
    return sorted(child for child in found if child.is_dir() and not child.is_symlink())


def idle_rows(tree, live, idle_hours, now):
    for directory in build_dirs(tree):
        size, newest = measure(directory)
        idle = (now - newest) / 3600
        if live[tree.path]:
            action, reason = 'keep', 'live process in worktree'
        elif idle < idle_hours:
            action, reason = 'keep', f'changed {idle:.1f} h ago, under {idle_hours:g} h'
        else:
            action, reason = 'remove', f'idle {idle:.1f} h, over {idle_hours:g} h'
        yield Row('build dir', directory, size, action, reason)


def plan(idle_hours=IDLE_HOURS, current=ROOT, root=ROOT, proc=Path('/proc'), now=None):
    now = time.time() if now is None else now
    trees = worktrees(root)
    live = live_processes(trees, proc)
    current = Path(current).resolve()

    def inspect(tree):
        decision = judge(tree, current, live, root)
        if tree.missing:
            return [Row('worktree', tree.path, 0, decision.action, decision.reason, tree.branch)]
        if decision.action == 'remove':
            size, _ = measure(tree.path)
            return [Row('worktree', tree.path, size, 'remove', decision.reason, tree.branch, decision.branch_force)]
        size, _ = measure(tree.path / '.local')
        return [
            Row('worktree', tree.path, size, 'keep', decision.reason, tree.branch),
            *idle_rows(tree, live, idle_hours, now),
        ]

    with ThreadPoolExecutor(8) as pool:
        return Plan([row for rows in pool.map(inspect, trees) for row in rows])


def display(path):
    home = str(Path.home())
    text = str(path)
    return '~' + text[len(home) :] if text.startswith(home + '/') else text


def table(plan_):
    header = ('kind', 'path', 'branch', 'size', 'action', 'why')
    body = [
        (
            row.kind,
            display(row.path),
            row.branch or '-',
            f'{row.size / GB:.2f} GB',
            'REMOVE' if row.action == 'remove' else 'keep',
            row.reason,
        )
        for row in plan_.rows
    ]
    widths = [max(len(line[i]) for line in [header, *body]) for i in range(len(header))]
    lines = [
        '  '.join(cell.ljust(width) for cell, width in zip(line, widths, strict=True)).rstrip()
        for line in [header, *body]
    ]
    return '\n'.join(lines)


def inside(path, root):
    return root in path.resolve().parents


def apply_row(row, root=ROOT):
    """Remove one planned row. Returns an error message, or None when it worked."""
    if row.kind == 'worktree':
        out = run('git', 'worktree', 'remove', str(row.path), cwd=root)  # no --force: git refuses a dirty tree
        if out.returncode:
            return out.stderr.strip()
        if row.branch:
            flag = '-D' if row.branch_force else '-d'
            out = run('git', 'branch', flag, row.branch, cwd=root)
            if out.returncode:
                return f'worktree removed, branch kept: {out.stderr.strip()}'
        return None
    local = row.path.parent
    if row.path.is_symlink() or local.name != '.local' or not inside(row.path, local.parent):
        return 'refusing a path outside <worktree>/.local'
    shutil.rmtree(row.path)
    return None


def free_gb(path=ROOT):
    return shutil.disk_usage(path).free / GB


def low_disk_warning(path=ROOT, minimum_gb=LOW_DISK_GB):
    """A one-line advice when the repository filesystem is nearly full, else None."""
    free = free_gb(path)
    if free >= minimum_gb:
        return None
    return (
        f'warning: only {free:.0f} GB free on the repository filesystem (under {minimum_gb} GB); run `mise run clean`'
    )


def warn_if_low_disk():
    if warning := low_disk_warning():
        print(warning, file=sys.stderr, flush=True)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--apply', action='store_true', help='perform the removals (default: dry run)')
    parser.add_argument('--idle-hours', type=float, default=IDLE_HOURS, help='delete build dirs unchanged this long')
    parser.add_argument('--no-fetch', action='store_true', help='do not fetch origin before judging merges')
    args = parser.parse_args(argv)

    if not args.no_fetch and run('git', 'fetch', 'origin', '--quiet').returncode:
        print('warning: git fetch origin failed; merges are judged against the last known origin/main', file=sys.stderr)
    before_free, before_count = free_gb(), len(worktrees())
    plan_ = plan(args.idle_hours)
    print(table(plan_))
    reclaim = sum(row.size for row in plan_.removable)
    print(
        f'\n{len(plan_.removable)} removals, {reclaim / GB:.1f} GB; {before_free:.0f} GB free, {before_count} worktrees'
    )
    if not args.apply:
        print('dry run: pass --apply to remove')
        return 0
    failures = 0
    for row in plan_.removable:
        error = apply_row(row)
        print(
            f'{"FAIL" if error else "ok  "} {row.kind} {display(row.path)}' + (f': {error}' if error else ''),
            flush=True,
        )
        failures += bool(error)
    print(f'after: {free_gb():.0f} GB free, {len(worktrees())} worktrees')
    return 1 if failures else 0


if __name__ == '__main__':
    sys.exit(main())
