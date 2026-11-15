#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
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
# `build.sh maintenance` (cycling build-maintenance) builds the MMC
# maintenance image instead of the firmware.
case "${1:-}" in
  "") firmware_features="hardware"; firmware_target="$target_root/firmware"; variant="" ;;
  maintenance) firmware_features="hardware,bulk-maintenance"; firmware_target="$target_root/firmware-maintenance"; variant="-maint" ;;
  # A measurement build that rides central Santiago on scripted fixes
  # (devices/c606/src/capabilities/scripted.rs); never for riders.
  scripted-gnss) firmware_features="hardware,scripted-gnss"; firmware_target="$target_root/firmware-gnss"; variant="-gnss" ;;
  *) echo "usage: build.sh [maintenance|scripted-gnss]" >&2; exit 2 ;;
esac
mkdir -p .local "$image_dir"
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
CYCLING_BUILD_COMMIT="$commit_placeholder" cargo build --target-dir "$firmware_target" -p c606-firmware \
  --bin cycling-os --release --locked --no-default-features --features "$firmware_features" \
  --target xtensa-esp32s3-none-elf
elf="$image_dir/cycling.elf"
uv run --project tools/devtools --locked python -m cycling_devtools.stamp \
  "$firmware_target/xtensa-esp32s3-none-elf/release/cycling-os" "$elf" \
  --placeholder "$commit_placeholder" --commit "$CYCLING_BUILD_COMMIT"
# The main stack is the internal DRAM that statics and the heap leave. Each
# task and VANA's frame preparation run on it, so growing statics silently
# shrinks it until boot overflows the stack guard (#167). Fail the build
# first: keep new state out of internal RAM, or measure STATUS
# stack_untouched on the device and lower the budget deliberately.
min_stack_bytes="${CYCLING_MIN_STACK_BYTES:-61440}"
stack_symbols="$(xtensa-esp32s3-elf-nm "$elf")"
stack_start="$(awk '$3 == "_stack_start_cpu0" { print $1 }' <<<"$stack_symbols")"
stack_end="$(awk '$3 == "_stack_end_cpu0" { print $1 }' <<<"$stack_symbols")"
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
    --report "$image_dir/stack-frames.md"
fi
espflash save-image --chip esp32s3 --flash-mode dio --flash-freq 80mhz --flash-size 16mb \
  --partition-table devices/c606/partitions.csv --target-app-partition ota_1 \
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
cp "$elf" "$archive/.$CYCLING_BUILD_COMMIT$suffix$variant.elf.$$"
mv -f "$archive/.$CYCLING_BUILD_COMMIT$suffix$variant.elf.$$" "$archive/$CYCLING_BUILD_COMMIT$suffix$variant.elf"
# Best effort: a concurrent build in another worktree may prune a file between
# the glob and ls, which must not fail this build.
{ ls -1t "$archive"/*.elf 2>/dev/null | tail -n +25 | xargs -r rm -f; } || true
