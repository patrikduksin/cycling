"""Free disk: remove merged worktrees and idle build directories (a dry run unless --apply).

Every linked worktree carries its own .local/ build targets, about 4.5 GB each, and
agents leave them behind. A worktree is removed only when it is provably dead: its
branch is merged, it has commits of its own, its tree is clean and quiet, it holds
no ignored file outside the regenerable build dirs, it contains no other worktree
and no process works inside it. Build directories of the worktrees that stay are
deleted when nothing in them changed for --idle-hours. The primary checkout and
its .local/elf archive are never touched. Every row is judged again right before
it is deleted.

Live agents are found by the cwd of each /proc/<pid>. Another user's processes
expose no cwd to us and are not seen, so run this as the user who runs the agents.
"""

import argparse
import enum
import fnmatch
import json
import math
import os
import shutil
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path

from cycling_devtools.workspace import ROOT

# Idle-deleted under <worktree>/.local: regenerable by build.sh and verify.
BUILD_DIR_PATTERNS = ('firmware*', 'verify')
# Ignored content that a removed worktree may take with it. Everything else ignored
# (agent notes, harness runs, sim/live recordings, the sim-compare checkout) blocks removal.
CACHE_NAMES = ('__pycache__', '.pytest_cache', '.ruff_cache', '.venv')
KEEP_IN_SIM = ('live', 'compare')
IDLE_HOURS = 24.0
LOW_DISK_GB = 30
GB = 10**9


class Action(enum.Enum):
    REMOVE = 'remove'
    KEEP = 'keep'


@dataclass(frozen=True)
class Worktree:
    path: Path
    head: str
    branch: str | None
    primary: bool
    locked: bool
    missing: bool


@dataclass(frozen=True)
class Process:
    pid: int
    command: str
    cwd: Path


@dataclass(frozen=True)
class Context:
    trees: list[Worktree]
    processes: list[Process]
    current: Path
    root: Path
    idle_hours: float
    now: float
    unreadable: int = 0  # processes whose cwd we may not read (other users)


@dataclass(frozen=True)
class Decision:
    action: Action
    reason: str
    branch_force: bool = False  # a squash-merged branch is not an ancestor of origin/main


@dataclass(frozen=True)
class Row:
    kind: str  # 'worktree' or 'build dir'
    path: Path
    size: int
    action: Action
    reason: str
    branch: str | None = None
    branch_force: bool = False


def keep(reason):
    return Decision(Action.KEEP, reason)


def run(*command, cwd=ROOT):
    return subprocess.run(command, cwd=cwd, capture_output=True, text=True, timeout=120)


def inside(path, root):
    return path == root or root in path.parents


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


def processes(proc=Path('/proc')):
    """(processes whose cwd we can read, count of those we cannot). Fails closed when /proc is unreadable."""
    if not (proc / str(os.getpid()) / 'cwd').exists():
        raise RuntimeError(f'cannot read {proc}/<pid>/cwd, so live agents cannot be detected')
    found, unreadable = [], 0
    for entry in proc.iterdir():
        if not entry.name.isdigit():
            continue
        try:
            cwd = Path(os.readlink(entry / 'cwd').removesuffix(' (deleted)'))
            command = (entry / 'cmdline').read_bytes().replace(b'\0', b' ').decode(errors='replace').strip()
        except PermissionError:
            unreadable += 1
            continue
        except OSError:
            continue
        found.append(Process(int(entry.name), command[:60], cwd))
    return found, unreadable


def context(idle_hours=IDLE_HOURS, current=ROOT, root=ROOT, proc=Path('/proc'), now=None):
    found, unreadable = processes(proc)
    return Context(
        worktrees(root),
        found,
        Path(current).resolve(),
        root,
        idle_hours,
        time.time() if now is None else now,
        unreadable,
    )


def occupants(path, ctx):
    """Processes with their cwd anywhere inside path, nested worktrees included."""
    return [process for process in ctx.processes if inside(process.cwd, path)]


def nested_trees(tree, ctx):
    return [other for other in ctx.trees if other.path != tree.path and inside(other.path, tree.path)]


def direct_occupants(tree, ctx):
    """Processes working in this worktree itself, not in a worktree nested inside it."""
    nested = nested_trees(tree, ctx)
    return [p for p in occupants(tree.path, ctx) if not any(inside(p.cwd, other.path) for other in nested)]


def dirty(path):
    """Uncommitted or untracked changes outside ignored paths; unknown counts as dirty."""
    out = run('git', 'status', '--porcelain', '--untracked-files=all', cwd=path)
    return out.returncode != 0 or bool(out.stdout.strip())


def is_build(parts):
    """Whether a path relative to the worktree is regenerable ignored content."""
    if any(name in CACHE_NAMES for name in parts):
        return True
    if parts[:1] == ('target',):
        return True
    if parts[:1] != ('.local',) or len(parts) < 2:
        return False
    if any(fnmatch.fnmatch(parts[1], pattern) for pattern in BUILD_DIR_PATTERNS):
        return True
    return parts[1] == 'sim' and len(parts) > 2 and parts[2] not in KEEP_IN_SIM


def foreign(root, relative):
    """The first ignored path under relative that a removal would destroy for good, else None."""
    parts = relative.parts
    if is_build(parts):
        return None
    path = root / relative
    if path.is_dir() and not path.is_symlink():
        for child in sorted(path.iterdir()):
            if found := foreign(root, relative / child.name):
                return found
        return None
    return relative


def foreign_ignored(path):
    """An ignored file or directory that is not regenerable, such as agent notes; None if there is none."""
    out = run('git', 'ls-files', '-o', '-i', '--exclude-standard', '--directory', '-z', cwd=path)
    if out.returncode:
        return Path('(ignored files unreadable)')
    for entry in filter(None, out.stdout.split('\0')):
        if found := foreign(path, Path(entry.rstrip('/'))):
            return found
    return None


def newest_outside_builds(path):
    """Newest mtime of the worktree's files other than regenerable build output."""
    newest = 0.0
    stack = [path]
    while stack:
        try:
            with os.scandir(stack.pop()) as entries:
                for entry in entries:
                    relative = Path(entry.path).relative_to(path).parts
                    if relative == ('.git',) or is_build(relative):
                        continue
                    newest = max(newest, entry.stat(follow_symlinks=False).st_mtime)
                    if entry.is_dir(follow_symlinks=False):
                        stack.append(entry.path)
        except OSError:
            continue
    return newest


def has_own_commits(path):
    """Whether anyone committed in this worktree; a freshly spawned agent's tree has not."""
    out = run('git', 'reflog', 'show', '--format=%gs', 'HEAD', cwd=path)
    return any(line.startswith('commit') for line in out.stdout.splitlines())


def unmerged_history(tree, root=ROOT):
    """A commit once at HEAD or the branch tip that origin/main lacks, else None.

    A commit followed by a reset to main leaves HEAD in origin/main but the commit
    reachable only from a reflog, which removing the worktree discards. Unreadable
    history counts as unmerged.
    """
    revs = ['HEAD', f'refs/heads/{tree.branch}'] if tree.branch else ['HEAD']
    hashes = set()
    for rev in revs:
        out = run('git', 'reflog', 'show', '--format=%H', rev, cwd=tree.path)
        if out.returncode:
            return 'unreadable reflog'
        hashes.update(out.stdout.split())
    if not hashes:
        return None
    out = run('git', 'rev-list', *sorted(hashes), '--not', 'origin/main', cwd=root)
    if out.returncode:
        return 'unreadable history'
    return out.stdout.split()[0][:7] if out.stdout.strip() else None


def in_main(rev, root=ROOT):
    return run('git', 'merge-base', '--is-ancestor', rev, 'origin/main', cwd=root).returncode == 0


def merged_pull_request(branch, head, root=ROOT):
    """The number of a merged PR whose tip is HEAD or contains it, else None.

    Matching the tip, not just the branch name, keeps commits pushed to a reused
    branch name after the merge from being treated as merged. Unreadable gh output
    means not merged.
    """
    out = run(
        'gh', 'pr', 'list', '--state', 'merged', '--head', branch,
        '--json', 'number,headRefOid', '--limit', '10',
        cwd=root,
    )  # fmt: skip
    if out.returncode:
        return None
    try:
        pulls = [(pull['number'], pull['headRefOid']) for pull in json.loads(out.stdout or '[]')]
    except ValueError, KeyError, TypeError:
        return None
    for number, tip in pulls:
        if tip == head or run('git', 'merge-base', '--is-ancestor', head, tip, cwd=root).returncode == 0:
            return number
    return None


def judge(tree, ctx):
    """Why a worktree stays, or the proof that it is dead. The first failing rule wins."""
    if tree.primary:
        return keep('primary checkout')
    if tree.path == ctx.current:
        return keep('current worktree')
    if live := occupants(tree.path, ctx):
        return keep(f'live process {live[0].pid} ({live[0].command})')
    if nested := nested_trees(tree, ctx):
        return keep(f'contains worktree {nested[0].path.name}')
    if tree.locked:
        return keep('locked')
    if tree.missing:
        return keep('directory missing (git worktree prune)')
    if dirty(tree.path):
        return keep('uncommitted or untracked changes')
    if found := foreign_ignored(tree.path):
        return keep(f'ignored files outside build dirs: {found}')
    quiet = (ctx.now - newest_outside_builds(tree.path)) / 3600
    if quiet < ctx.idle_hours:
        return keep(f'files changed {quiet:.1f} h ago, under {ctx.idle_hours:g} h')
    if in_main(tree.head, ctx.root):
        if not has_own_commits(tree.path):
            return keep('no commits of its own (may be a new agent)')
        if lost := unmerged_history(tree, ctx.root):
            return keep(f'reflog holds {lost}, which origin/main lacks')
        return Decision(Action.REMOVE, 'HEAD is in origin/main, tree clean and quiet, no live process')
    if tree.branch and (number := merged_pull_request(tree.branch, tree.head, ctx.root)):
        return Decision(Action.REMOVE, f'PR #{number} merged, tree clean and quiet, no live process', True)
    return keep('not merged')


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


def judge_dir(tree, directory, ctx):
    """(size, decision) for one build dir of a kept worktree."""
    size, newest = measure(directory)
    idle = (ctx.now - newest) / 3600
    if direct_occupants(tree, ctx) or occupants(directory, ctx):
        return size, keep('live process in worktree')
    if idle < ctx.idle_hours:
        return size, keep(f'changed {idle:.1f} h ago, under {ctx.idle_hours:g} h')
    return size, Decision(Action.REMOVE, f'idle {idle:.1f} h, over {ctx.idle_hours:g} h')


def rejudge(row, ctx):
    """The decision for a planned row against the machine as it is now."""
    tree = next((tree for tree in ctx.trees if tree.path == row.path), None)
    if row.kind == 'worktree':
        return judge(tree, ctx) if tree else keep('no longer a registered worktree')
    owner = next((tree for tree in ctx.trees if tree.path == row.path.parent.parent), None)
    if owner is None or not row.path.is_dir():
        return keep('no longer present')
    return judge_dir(owner, row.path, ctx)[1]


def plan(ctx):
    def inspect(tree):
        decision = judge(tree, ctx)
        if tree.missing:
            return [Row('worktree', tree.path, 0, decision.action, decision.reason, tree.branch)]
        if decision.action is Action.REMOVE:
            size, _ = measure(tree.path)
            return [
                Row('worktree', tree.path, size, decision.action, decision.reason, tree.branch, decision.branch_force)
            ]
        size, _ = measure(tree.path / '.local')
        rows = [Row('worktree', tree.path, size, decision.action, decision.reason, tree.branch)]
        for directory in build_dirs(tree):
            size, verdict = judge_dir(tree, directory, ctx)
            rows.append(Row('build dir', directory, size, verdict.action, verdict.reason))
        return rows

    with ThreadPoolExecutor(8) as pool:
        return [row for rows in pool.map(inspect, ctx.trees) for row in rows]


def removable(rows):
    return [row for row in rows if row.action is Action.REMOVE]


def display(path):
    home = str(Path.home())
    text = str(path)
    return '~' + text[len(home) :] if text.startswith(home + '/') else text


def table(rows):
    header = ('kind', 'path', 'branch', 'size', 'action', 'why')
    body = [
        (
            row.kind,
            display(row.path),
            row.branch or '-',
            f'{row.size / GB:.2f} GB',
            row.action.value.upper() if row.action is Action.REMOVE else row.action.value,
            row.reason,
        )
        for row in rows
    ]
    widths = [max(len(line[i]) for line in [header, *body]) for i in range(len(header))]
    lines = [
        '  '.join(cell.ljust(width) for cell, width in zip(line, widths, strict=True)).rstrip()
        for line in [header, *body]
    ]
    return '\n'.join(lines)


def apply_row(row, root=ROOT):
    """Remove one planned row. Returns an error message, or None when it worked."""
    if row.kind == 'worktree':
        out = run('git', 'worktree', 'remove', str(row.path), cwd=root)  # no --force: git refuses a dirty tree
        if out.returncode:
            return out.stderr.strip()
        if row.branch:
            if not (row.branch_force or in_main(row.branch, root)):
                return 'worktree removed, branch kept: not merged into origin/main'
            out = run('git', 'branch', '-D', row.branch, cwd=root)
            if out.returncode:
                return f'worktree removed, branch kept: {out.stderr.strip()}'
        return None
    local = row.path.parent
    if row.path.is_symlink() or local.name != '.local' or not inside(row.path.resolve(), local.parent):
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
    return f'warning: only {math.floor(free)} GB free on the repository filesystem (under {minimum_gb} GB); run `mise run clean`'


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
    ctx = context(args.idle_hours)
    rows = plan(ctx)
    print(table(rows))
    if ctx.unreadable:
        print(f'note: {ctx.unreadable} processes of other users have an unreadable cwd and are not checked')
    reclaim = sum(row.size for row in removable(rows))
    print(
        f'\n{len(removable(rows))} removals, {reclaim / GB:.1f} GB; {before_free:.0f} GB free, {before_count} worktrees'
    )
    if not args.apply:
        print('dry run: pass --apply to remove')
        return 0
    failures = 0
    for row in removable(rows):
        verdict = rejudge(row, context(args.idle_hours))
        if verdict.action is Action.KEEP:
            print(f'skip {row.kind} {display(row.path)}: now {verdict.reason}', flush=True)
            continue
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
