#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
# The flake dev shell (`nix develop`) provides the pinned Xtensa toolchain.
if [[ -z "${CYCLING_ESP_TOOLCHAIN:-}" ]]; then
  echo "Enter the dev shell first: nix develop (docs/dev-setup.md)." >&2
  exit 1
fi
for retired in CYCLING_SDK CYCLING_HARNESS CYCLING_BULK_MAINTENANCE; do
  if [[ -n "${!retired:-}" ]]; then
    echo "$retired is retired: every build has VANA and the harness; cycling build-maintenance builds the maintenance image." >&2
    exit 1
  fi
done
# Target dirs default to this checkout; point the root elsewhere to share them.
target_root="${CYCLING_FW_TARGET_ROOT:-$PWD/.local}"
# The image and its ELF land in .local, where flash and backtrace read them;
# a build beside another (verify) needs its own directory.
image_dir="${CYCLING_IMAGE_DIR:-.local}"
variant=""
case "$#:${1:-}" in
  0:) ;;
  2:--variant) variant="$2" ;;
  *) echo "usage: build.sh [--variant NAME]" >&2; exit 2 ;;
esac
mkdir -p .local "$image_dir"
partitions="$image_dir/partitions.csv"
manifest="$(uv run --project tools/devtools --locked python -m cycling_devtools.devices --build-env "$partitions" --variant "$variant")"
eval "$manifest"
firmware_target="$target_root/firmware${variant:+-$variant}"
export PATH="$CYCLING_ESP_TOOLCHAIN/bin:$CYCLING_ESP_TOOLCHAIN/xtensa-esp-elf/bin:$PATH"
CYCLING_BUILD_COMMIT="$(git rev-parse --short=12 HEAD)"
placeholder_source="zcyclingzcommitzplaceholderzzzzzzzzzzzzzzzzzzzzzzzzz"
commit_placeholder="${placeholder_source:0:${#CYCLING_BUILD_COMMIT}}"
if [[ -n "$(git status --porcelain --untracked-files=no)" ]]; then
  export CYCLING_BUILD_DIRTY=1
else
  export CYCLING_BUILD_DIRTY=0
fi
# sccache keys every rustc call on all CARGO_* variables: an exported
# per-worktree CARGO_TARGET_DIR would keep every worktree's builds apart.
unset CARGO_TARGET_DIR
CYCLING_BUILD_COMMIT="$commit_placeholder" cargo build --target-dir "$firmware_target" -p "$package" \
  --bin "$bin" --release --locked --no-default-features --features "$features" \
  --target "$target"
elf="$image_dir/cycling.elf"
uv run --project tools/devtools --locked python -m cycling_devtools.stamp \
  "$firmware_target/$target/release/$bin" "$elf" \
  --placeholder "$commit_placeholder" --commit "$CYCLING_BUILD_COMMIT"
# The main stack is the internal DRAM that statics and the heap leave. Each
# task and VANA's frame preparation run on it, so growing statics silently
# shrinks it until boot overflows the stack guard (#167). Fail the build
# first: keep new state out of internal RAM, or measure STATUS
# stack_untouched on the device and lower the budget deliberately.
min_stack_bytes="${CYCLING_MIN_STACK_BYTES:-$stack_budget}"
stack_symbols="$("$nm" "$elf")"
stack_start="$(awk -v symbol="$stack_start_symbol" '$3 == symbol { print $1 }' <<<"$stack_symbols")"
stack_end="$(awk -v symbol="$stack_end_symbol" '$3 == symbol { print $1 }' <<<"$stack_symbols")"
stack_bytes=$((16#$stack_start - 16#$stack_end))
echo "Main stack region: $stack_bytes bytes (budget $min_stack_bytes)"
if (( stack_bytes < min_stack_bytes )); then
  echo "The main stack region is below its budget: internal DRAM statics grew by $((min_stack_bytes - stack_bytes)) bytes too many." >&2
  exit 1
fi
# The region's size says nothing of what one frame takes from it (#205 found
# a 30 KB console frame). Each heavy frame has a budget in stack-frames.toml;
# a frame past its budget fails the build, naming the function.
# The maintenance image runs no VANA frames and has no budgets.
if [[ -z "$variant" ]]; then
  uv run --project tools/devtools --locked python -m cycling_devtools.stack_frames "$elf" \
    --budgets "$device_dir/stack-frames.toml" --report "$image_dir/stack-frames.md"
fi
espflash save-image --chip "$chip" --flash-mode "$flash_mode" --flash-freq "$flash_freq" --flash-size "$flash_size" \
  --partition-table "$partitions" --target-app-partition "$app_partition" \
  "$elf" "$image_dir/cycling.bin"
# And one per commit and image, named as the panic record's elf= says, in
# the primary checkout's .local so every worktree's builds are found
# (cycling backtrace). Written under a temporary name, then moved, so
# parallel builds never leave a torn file; the newest 24 are kept.
suffix=""
if [[ "$CYCLING_BUILD_DIRTY" == 1 ]]; then
  suffix="-dirty"
fi
archive="$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")/.local/elf"
mkdir -p "$archive"
name="$CYCLING_BUILD_COMMIT$suffix${variant:+-$variant}"
cp "$elf" "$archive/.$name.elf.$$"
mv -f "$archive/.$name.elf.$$" "$archive/$name.elf"
# Best effort: a concurrent build in another worktree may prune a file between
# the glob and ls, which must not fail this build.
{ ls -1t "$archive"/*.elf 2>/dev/null | tail -n +25 | xargs -r rm -f; } || true
