#!/usr/bin/env bash
# SessionStart: load the flake dev shell into every Bash command of an agent
# session. Agent shells are non-interactive, so direnv's prompt hook never runs.
cd "${CLAUDE_PROJECT_DIR:-.}" || exit 0
if ! command -v direnv >/dev/null || [[ -z "${CLAUDE_ENV_FILE:-}" ]]; then
  echo "No direnv here: run scripts/bootstrap.sh, or prefix commands with 'nix develop -c'." >&2
  exit 0
fi
# The first build of a new flake.lock can take minutes; later sessions hit nix-direnv's cache.
direnv export bash >>"$CLAUDE_ENV_FILE" 2>/dev/null || echo "direnv failed; run 'direnv allow' in $PWD" >&2
