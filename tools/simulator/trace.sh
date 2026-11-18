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

json=false
for arg; do [ "$arg" != --json ] || json=true; done
if $json; then
    set -E
    trap 'echo "{\"error\": \"the trace launcher failed at line $LINENO; see stderr\"}"; exit 2' ERR
fi

{
    read -r root
    read -r common
    read -r commit
} < <(git rev-parse --show-toplevel --path-format=absolute --git-common-dir HEAD)

primary=$(dirname "$common")
export CYCLING_TERRAIN_PACKS="${CYCLING_TERRAIN_PACKS:-$primary/.local/terrain}"

if [ "${1:-}" = replay ]; then
    for arg; do
        if [ "$arg" = --against ]; then
            exec uv run --project "$root/tools/devtools" --locked \
                python -m cycling_devtools.twin against "$@"
        fi
    done
fi

if [ "${1:-}" = nightly ]; then
    shift
    exec uv run --project "$root/tools/devtools" --locked \
        python -m cycling_devtools.nightly "$@"
fi

if [ "${1:-}" = upgrade ]; then
    shift
    exec uv run --project "$root/tools/devtools" --locked \
        python -m cycling_devtools.upgrade "$@"
fi

if [ "${1:-}" = pull ]; then
    shift
    exec uv run --project "$root/tools/devtools" --locked \
        python -m cycling_devtools.device.volumes trace-pull "$@"
fi

mapfile -t sources < "$root/tools/simulator/build-sources"
edited() {
    [ -n "$(git -C "$root" status --porcelain --untracked-files=normal -- "${sources[@]}")" ]
}
cache=$(dirname "$common")/.local/tools/trace
mkdir -p "$cache"

# A failed build under --json still prints one JSON document; cargo's
# diagnostics went to stderr.
build_failed() {
    rm -f "$copy"
    if $json; then
        echo '{"error": "the trace tool did not build; cargo said why on stderr"}'
        exit 2
    fi
    exit "$1"
}

# Worktrees can share a target dir, so the build and the copy out of it hold
# one lock: the copy is then this build, not another worktree's. Cargo names
# the target dir and the binary, whatever its config says. Cargo gets neither
# lock's fd, or the sccache server it starts would hold it for good.
build_copy() {
    copy=$(mktemp "$cache/.$commit.XXXXXX")
    local target built
    target=$(cargo metadata --format-version 1 --no-deps --manifest-path "$root/Cargo.toml" 8>&- \
        | jq -r .target_directory) || build_failed $?
    mkdir -p "$target"
    {
        flock 9 \
            && built=$(cargo build -q --manifest-path "$root/Cargo.toml" --profile tools -p shell-simulator \
                --locked --no-default-features --bin trace --message-format=json-render-diagnostics 8>&- 9>&- \
                | jq -r 'select(.reason == "compiler-artifact" and .target.name == "trace") | .executable // empty') \
            && install -m 0755 "$built" "$copy"
    } 9> "$target/.trace-build.lock" || build_failed $?
}
copy_is_the_commits() {
    [ "$(git -C "$root" rev-parse HEAD)" = "$commit" ] && ! edited \
        && uv run --project "$root/tools/devtools" --locked python -m cycling_devtools.build_hash \
            built-from "$("$copy" --build-hash)" "$commit"
}
run_copy() {
    status=0
    "$copy" "$@" || status=$?
    rm -f "$copy"
    exit $status
}

if edited; then
    build_copy
    run_copy "$@"
fi

# A cached binary runs holding a shared lock on fd 8, kept across the exec,
# and pruning skips while anyone holds it.
exec 8> "$cache/.lock"
# Each pass runs the cached binary or builds it; a binary just cached runs on
# the next pass, under the shared lock like any other.
while :; do
    flock -s 8
    # The touch marks the binary as used, so pruning by age keeps the ones in use.
    if [ -x "$cache/$commit" ] && touch -c "$cache/$commit"; then
        exec "$cache/$commit" "$@"
    fi
    flock -u 8
    build_copy
    copy_is_the_commits || run_copy "$@"
    mv -f "$copy" "$cache/$commit"
    (
        flock -xn 7 || exit 0
        # A .from note is not a binary: it goes with the binary it describes.
        ls -t "$cache" | grep -v '\.from$' | tail -n +6 \
            | (cd "$cache" && while read -r old; do rm -f -- "$old" "$old.from"; done)
        find "$cache" -maxdepth 1 -name '.*.??????*' -mmin +60 -delete
    ) 7> "$cache/.lock" || true
done
