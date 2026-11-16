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
target=${CARGO_TARGET_DIR:-$root/target}
[[ $target = /* ]] || target=$PWD/$target
cache=$(dirname "$common")/.local/tools/trace
mkdir -p "$target" "$cache"

# Worktrees can share a target dir, so the build and the copy out of it hold
# one lock: the copy is then this build, not another worktree's. Cargo gets
# neither lock's fd, or the sccache server it starts would hold it for good.
build_copy() {
    copy=$(mktemp "$cache/.$commit.XXXXXX")
    {
        flock 9 \
            && cargo build -q --manifest-path "$root/Cargo.toml" --profile tools -p shell-simulator \
                --locked --no-default-features --bin trace 8>&- 9>&- \
            && install -m 0755 "$target/tools/trace" "$copy"
    } 9> "$target/.trace-build.lock" || {
        status=$?
        rm -f "$copy"
        exit $status
    }
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
while :; do
    flock -s 8
    # The touch marks the binary as used, so pruning by age keeps the ones in use.
    if [ -x "$cache/$commit" ] && touch -c "$cache/$commit"; then
        exec "$cache/$commit" "$@"
    fi
    flock -u 8
    build_copy
    # A commit or an edit made while it built would be in the binary; the
    # binary names the commit it was built from.
    if [ "$(git -C "$root" rev-parse HEAD)" != "$commit" ] || edited \
        || [ "$("$copy" --build-commit)" != "$commit" ]; then
        run_copy "$@"
    fi
    mv -f "$copy" "$cache/$commit"
    (
        flock -xn 7 || exit 0
        ls -t "$cache" | tail -n +6 | (cd "$cache" && xargs -r rm -f --)
        find "$cache" -maxdepth 1 -name '.*.??????' -mmin +60 -delete
    ) 7> "$cache/.lock" || true
done
