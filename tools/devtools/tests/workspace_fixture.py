"""A temporary git checkout whose ignored .local/ satisfies the private-path guard."""

import contextlib
from pathlib import Path
import subprocess
from unittest.mock import patch

from cycling_devtools import workspace


def init_checkout(root):
    root = Path(root)
    subprocess.run(['git', 'init', '-q', str(root)], check=True)
    (root / '.gitignore').write_text('/.local/\n')
    (root / '.local').mkdir(exist_ok=True)
    return root


@contextlib.contextmanager
def private_root(root):
    """Make root the workspace for private_path; yields its .local directory."""
    root = init_checkout(root)
    with patch.object(workspace, 'ROOT', root):
        yield root / '.local'
