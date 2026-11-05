#!/bin/sh
# The page flasher end to end with no device and no network: builds the
# simulator's browser bundle and the site against the offline relay, then
# runs e2e-page.ts inside offline-smoke.sh's loopback-only namespace.
# Screenshots go to $1 (default: a temporary directory that is printed).
#
#   sh web/agent/test/e2e-page.sh [shots-dir]
set -eu

ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
SHOTS=${1:-$WORK/shots}
mkdir -p "$SHOTS"

cd "$ROOT"
bun build web/flasher/test/sim/browser.ts --target=browser --format=iife --outfile="$WORK/sim.js" >/dev/null
VITE_RELAY_URL=ws://localhost:1337 bun run --cwd web/site build -- --outDir "$WORK/dist" --emptyOutDir >/dev/null
uv run --project tools/devtools --locked python -c '' >/dev/null

E2E_DIST="$WORK/dist" E2E_SIM="$WORK/sim.js" E2E_SHOTS="$SHOTS" \
  sh web/relay/test/offline-smoke.sh web/agent/test/e2e-page.ts
echo "screenshots in $SHOTS"
