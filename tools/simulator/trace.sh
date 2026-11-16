#!/usr/bin/env bash
# `cycling trace` and `cycling replay`: run the trace tool without cargo's
# freshness check. A checkout whose build sources match its commit runs the
# binary built for that commit, kept in the primary checkout's
# .local/tools/trace/ for every worktree and built once. One with
# uncommitted changes to them builds its own tree and runs that.
set -euo pipefail

{
    read -r root
    read -r common
    read -r commit
} < <(git rev-parse --show-toplevel --path-format=absolute --git-common-dir HEAD)
mapfile -t sources < "$root/tools/simulator/build-sources"
edited() {
    [ -n "$(git -C "$root" status --porcelain --untracked-files=no -- "${sources[@]}")" ]
}
build() {
    cargo build -q --manifest-path "$root/Cargo.toml" --profile tools -p shell-simulator \
        --locked --no-default-features --bin trace
}
target=${CARGO_TARGET_DIR:-$root/target}
[[ $target = /* ]] || target=$PWD/$target

if edited; then
    build
    exec "$target/tools/trace" "$@"
fi

cache=$(dirname "$common")/.local/tools/trace
if [ ! -x "$cache/$commit" ]; then
    build
    # An edit made while it built would be in the binary.
    if edited; then
        exec "$target/tools/trace" "$@"
    fi
    mkdir -p "$cache"
    copy=$(mktemp "$cache/.$commit.XXXXXX")
    install -m 0755 "$target/tools/trace" "$copy"
    mv -f "$copy" "$cache/$commit"
    ls -t "$cache" | tail -n +6 | (cd "$cache" && xargs -r rm -f --)
fi
exec "$cache/$commit" "$@"
