"""Repository paths and the private-evidence rule shared by host tools."""

from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[4]


def private_path(value):
    """Resolve value and require it strictly inside the git-ignored ROOT/.local.

    Symlinks are resolved first, so a link inside .local cannot escape it. The
    .local directory itself is refused; evidence belongs in a named child. Git
    must confirm the path is ignored, so a tracked file or a changed .gitignore
    cannot publish evidence. Any uncertainty fails closed.
    """
    local = (ROOT / '.local').resolve()
    path = Path(value).resolve()
    if local not in path.parents:
        raise ValueError('private evidence must stay inside ignored .local/')
    relative = Path('.local') / path.relative_to(local)
    try:
        ignored = (
            subprocess.run(
                ['git', 'check-ignore', '-q', '--', str(relative)],
                cwd=ROOT,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            ).returncode
            == 0
        )
    except OSError:
        ignored = False
    if not ignored:
        raise ValueError('private evidence path must be git-ignored')
    return path


def private_directory(value):
    """Create a new owner-only directory that satisfies private_path."""
    path = private_path(value)
    path.mkdir(parents=True, exist_ok=False, mode=0o700)
    path.chmod(0o700)
    return path
