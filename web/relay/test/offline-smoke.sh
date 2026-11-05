#!/bin/sh
# Starts the relay with `alchemy dev` and runs smoke.ts against it, with no way
# to reach Cloudflare: a private network namespace that has only loopback, and
# an empty HOME so no Alchemy profile or credential is readable. State is local
# disk under .alchemy/. Linux only (needs `unshare` and `ip`). An optional
# script replaces smoke.ts; it gets the relay URL as its first argument.
# Afterwards a second relay with a too-short AGENT_TOKEN must admit no agent.
#
#   sh web/relay/test/offline-smoke.sh [web/agent/test/e2e-relay.ts]
set -eu

ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
mkdir "$WORK/home"

cd "$ROOT"
unshare --net --map-root-user --pid --fork --kill-child sh -eu -c '
  ip link set lo up
  export HOME="$1/home" ORIGIN=http://localhost:5173
  relay() {
    AGENT_TOKEN="$2" bun node_modules/.bin/alchemy dev -c web/relay/dev.run.ts --stage "$1" >"$WORK/$1.log" 2>&1 &
    until grep -q "ready at" "$WORK/$1.log"; do
      kill -0 $! 2>/dev/null || { cat "$WORK/$1.log"; exit 1; }
      sleep 0.5
    done
    sed -n "s/.*ready at \(http[^ ]*\).*/\1/p" "$WORK/$1.log" | head -n 1
  }
  WORK="$1"
  URL=$(relay dev_relay_smoke smoke-agent-token-of-32-characters)
  echo "relay at $URL (offline, loopback only)"
  AGENT_TOKEN=smoke-agent-token-of-32-characters bun "$2" "$URL"
  SHORT=$(relay dev_relay_smoke_short short-token)
  echo "relay with a short AGENT_TOKEN at $SHORT"
  AGENT_TOKEN=short-token bun web/relay/test/smoke.ts "$SHORT" --bad-token
  grep -h "AGENT_TOKEN" "$WORK/dev_relay_smoke_short.log" | head -n 3
' sh "$WORK" "${1:-web/relay/test/smoke.ts}"
