#!/usr/bin/env bash
# SessionStart: load the flake dev shell into every Bash command of an agent
# session. CLAUDE_ENV_FILE is sourced before each command, so it gets the
# shell's final exports only: not nix's setup functions or its per-shell TMPDIR.
cd "${CLAUDE_PROJECT_DIR:-.}" || exit 0
if [[ -z "${CLAUDE_ENV_FILE:-}" ]]; then
  exit 0
fi
# The first build of a new flake.lock can take minutes; later sessions take
# about 2 s. The profile is a GC root, so a nix GC keeps the toolchains.
mkdir -p .local
nix develop --profile .local/dev-shell . --command bash -c \
  'unset TMP TMPDIR TEMP TEMPDIR NIX_BUILD_TOP shellHook; export -n PWD OLDPWD SHLVL _; export -p' \
  >>"$CLAUDE_ENV_FILE" || echo "nix develop failed in $PWD; prefix commands with 'nix develop -c'." >&2
