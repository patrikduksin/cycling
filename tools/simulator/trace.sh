#!/usr/bin/env bash
# `cycling trace` and `cycling replay`: trace.sh trace|replay [--] ARGS.
set -euo pipefail

task=${1:?usage: trace.sh trace|replay [--] args}
shift
[ "${1:-}" != -- ] || shift
case $task in
    trace) ;;
    replay) set -- replay "$@" ;;
    *)
        echo "trace.sh: no task $task; it runs trace or replay" >&2
        exit 2
        ;;
esac

{
    read -r root
    read -r common
    read -r commit
} < <(git rev-parse --show-toplevel --path-format=absolute --git-common-dir HEAD)

if [ "${1:-}" = pull ]; then
    shift
    exec uv run --project "$root/tools/devtools" --locked \
        python -m cycling_devtools.device.volumes trace-pull "$@"
fi

mapfile -t sources < "$root/tools/simulator/build-sources"
edited() {
    [ -n "$(git -C "$root" status --porcelain --untracked-files=normal -- "${sources[@]}")" ]
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
# The touch marks the binary as used, so pruning by age keeps the ones in use.
if [ -x "$cache/$commit" ] && touch -c "$cache/$commit"; then
    exec "$cache/$commit" "$@"
fi
build
# A commit or an edit made while it built would be in the binary.
if [ "$(git -C "$root" rev-parse HEAD)" != "$commit" ] || edited; then
    exec "$target/tools/trace" "$@"
fi
mkdir -p "$cache"
copy=$(mktemp "$cache/.$commit.XXXXXX")
install -m 0755 "$target/tools/trace" "$copy"
mv -f "$copy" "$cache/$commit"
ls -t "$cache" | tail -n +6 | (cd "$cache" && xargs -r rm -f --)
find "$cache" -maxdepth 1 -name '.*.??????' -mmin +60 -delete
exec "$cache/$commit" "$@"
