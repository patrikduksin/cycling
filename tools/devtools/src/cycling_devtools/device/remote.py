"""Run a device task on the machine the C606 is plugged into, from any machine.

    mise run device <task> [args...]      # e.g. device flash, device logs STATUS

CYCLING_DEVICE_HOST names that machine's ssh alias. This checkout, including
uncommitted and untracked files, is snapshotted as a commit and pushed into a
worktree of the device host's primary checkout, so its lock, backup and safety
rules are the ones that guard the device. `flash` builds here and sends only the
image; every other task sends nothing but the source. Afterwards the host's
.local evidence (logs, test reports, exports) is copied back to the same paths
here. Only the image and text cross the network: no USB forwarding.
"""

import hashlib
import os
import shlex
import socket
import subprocess
import sys
import tempfile

from cycling_devtools.workspace import ROOT

# Where the device host keeps its primary checkout, relative to its home.
CHECKOUT = os.environ.get('CYCLING_DEVICE_CHECKOUT', 'projects/cycling')
# Settings that describe this machine, not the task.
LOCAL_ONLY = {
    'CYCLING_DEVICE_HOST',
    'CYCLING_DEVICE_CHECKOUT',
    'CYCLING_ESP_TOOLCHAIN',
    'CYCLING_FW_TARGET_ROOT',
    'CYCLING_IMAGE_DIR',
}
# Build products stay where they were built; the host keeps its own private state.
NOT_RETURNED = ['/cycling.bin', '/cycling.elf', '/firmware*/', '/remote/', '/verify/', '/sim/', '/usb.lock']


def git(*args, env=None):
    return subprocess.run(['git', *args], cwd=ROOT, env=env, capture_output=True, text=True, check=True).stdout.strip()


def snapshot():
    """A commit of the working tree as it is now, untracked files included."""
    head = git('rev-parse', 'HEAD')
    with tempfile.TemporaryDirectory() as scratch:
        env = os.environ | {'GIT_INDEX_FILE': os.path.join(scratch, 'index')}
        git('read-tree', 'HEAD', env=env)
        git('add', '-A', env=env)
        tree = git('write-tree', env=env)
    if tree == git('rev-parse', 'HEAD^{tree}'):
        return head
    return git('commit-tree', tree, '-p', head, '-m', 'device task snapshot')


def slot():
    """One host worktree per source checkout, so parallel agents never share one."""
    digest = hashlib.sha256(f'{socket.gethostname()}:{ROOT}'.encode()).hexdigest()[:10]
    return f'{socket.gethostname()}-{digest}'


def task_environment():
    return {k: v for k, v in os.environ.items() if k.startswith('CYCLING_') and k not in LOCAL_ONLY}


def prepare(sha, worktree):
    """Check the snapshot out in the host's worktree for this checkout."""
    return f"""set -e
cd ~/{CHECKOUT}
if [ -d {worktree} ]; then git -C {worktree} checkout -q -f --detach {sha}
else git worktree add -q -f --detach {worktree} {sha}; fi
mkdir -p {worktree}/.local
"""


def run(worktree, task, args):
    """Run the task in the host's dev shell, with this machine's CYCLING_* task settings."""
    env = ' '.join(f'{k}={shlex.quote(v)}' for k, v in sorted(task_environment().items()))
    command = shlex.join(['mise', 'run', task, *args])
    return f"""cd ~/{CHECKOUT}/{worktree}
if command -v direnv >/dev/null; then shell='direnv exec .'; else shell='nix develop --command'; fi
exec $shell env {env} {command}
"""


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    host = os.environ.get('CYCLING_DEVICE_HOST')
    if not argv:
        sys.exit(__doc__)
    if not host:
        sys.exit('CYCLING_DEVICE_HOST is not set: run the task directly on the machine with the device.')
    task, args = argv[0], argv[1:]
    if task in {'build', 'flash-image'}:
        sys.exit(f'run `mise run device flash`: it builds here and flashes there; {task} is not a device task.')

    if task == 'flash':
        # Build here, where the toolchain and caches are; the host only writes the image.
        subprocess.run(['mise', 'run', 'build'], cwd=ROOT, check=True)
        task = 'flash-image'
    sha = snapshot()
    name = slot()
    worktree = f'.local/remote/{name}'
    remote = f'{host}:{CHECKOUT}'
    print(f'device task {task} on {host} at {sha[:12]}', file=sys.stderr, flush=True)
    subprocess.run(['git', 'push', '-q', '-f', remote, f'{sha}:refs/remote-device/{name}'], cwd=ROOT, check=True)

    ssh = ['ssh', '-o', 'BatchMode=yes', *(['-t'] if sys.stdin.isatty() else []), host]
    if task == 'flash-image':
        subprocess.run([*ssh, prepare(sha, worktree)], check=True)
        images = [str(ROOT / '.local' / image) for image in ('cycling.bin', 'cycling.elf')]
        subprocess.run(['rsync', '-a', *images, f'{remote}/{worktree}/.local/'], check=True)
        code = subprocess.run([*ssh, run(worktree, task, args)]).returncode
    else:
        code = subprocess.run([*ssh, prepare(sha, worktree) + run(worktree, task, args)]).returncode

    excludes = [f'--exclude={pattern}' for pattern in NOT_RETURNED]
    (ROOT / '.local').mkdir(exist_ok=True)
    subprocess.run(['rsync', '-a', *excludes, f'{remote}/{worktree}/.local/', str(ROOT / '.local') + '/'], check=False)
    return code


if __name__ == '__main__':
    sys.exit(main())
