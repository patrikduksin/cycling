"""Helpers for the verify and merge tests' throwaway checkout (the `checkout` fixture in conftest.py)."""

import subprocess

IDENTITY = {
    'GIT_CONFIG_GLOBAL': '/dev/null',
    'GIT_CONFIG_NOSYSTEM': '1',
    'GIT_AUTHOR_NAME': 'Rider',
    'GIT_AUTHOR_EMAIL': 'rider@example.invalid',
    'GIT_COMMITTER_NAME': 'Rider',
    'GIT_COMMITTER_EMAIL': 'rider@example.invalid',
}


def git(*args, cwd):
    return subprocess.run(['git', *args], cwd=cwd, check=True, capture_output=True, text=True).stdout.strip()


def commit(root, message, **files):
    for name, text in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    git('add', '-A', cwd=root)
    git('commit', '-qm', message, cwd=root)
    return git('rev-parse', 'HEAD', cwd=root)


def gate_runs(root):
    log = root.parent / 'runs.log'
    return len(log.read_text().splitlines()) if log.exists() else 0
