#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
# The flake dev shell (direnv or `nix develop`) provides the pinned Xtensa toolchain.
if [[ -z "${CYCLING_ESP_TOOLCHAIN:-}" ]]; then
  echo "Enter the dev shell first: direnv allow, or nix develop (docs/dev-setup.md)." >&2
  exit 1
fi
# Target dirs default to this checkout; point the root elsewhere to share them.
target_root="${CYCLING_FW_TARGET_ROOT:-$PWD/.local}"
# The image and its ELF land in .local, where flash and backtrace read them;
# parallel builds of different variants each need their own directory.
image_dir="${CYCLING_IMAGE_DIR:-.local}"
case "${CYCLING_HARNESS:-1}" in
  1) firmware_features="hardware,debug-harness"; firmware_target="$target_root/firmware" ;;
  0) firmware_features="hardware"; firmware_target="$target_root/firmware-no-harness" ;;
  *) echo "CYCLING_HARNESS must be 0 or 1." >&2; exit 1 ;;
esac
case "${CYCLING_SDK:-0}" in
  0) ;;
  1) firmware_features="$firmware_features,cycling"; firmware_target="$firmware_target-sdk" ;;
  *) echo "CYCLING_SDK must be 0 or 1." >&2; exit 1 ;;
esac
case "${CYCLING_BULK_MAINTENANCE:-0}" in
  0) ;;
  1) firmware_features="$firmware_features,bulk-maintenance"; firmware_target="$firmware_target-maintenance" ;;
  *) echo "CYCLING_BULK_MAINTENANCE must be 0 or 1." >&2; exit 1 ;;
esac
echo "Building with USB test harness=${CYCLING_HARNESS:-1}"
mkdir -p .local "$image_dir"
export PATH="$CYCLING_ESP_TOOLCHAIN/bin:$CYCLING_ESP_TOOLCHAIN/xtensa-esp-elf/bin:$PATH"
CYCLING_BUILD_COMMIT="$(git rev-parse --short=12 HEAD)"
export CYCLING_BUILD_COMMIT
if [[ -n "$(git status --porcelain --untracked-files=no)" ]]; then
  export CYCLING_BUILD_DIRTY=1
else
  export CYCLING_BUILD_DIRTY=0
fi
export CARGO_TARGET_DIR="$firmware_target"
elf="$CARGO_TARGET_DIR/xtensa-esp32s3-none-elf/release/cycling-os"
cargo build -p c606-firmware --bin cycling-os --release --locked --no-default-features --features "$firmware_features" --target xtensa-esp32s3-none-elf
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
espflash save-image --chip esp32s3 --flash-mode dio --flash-freq 80mhz --flash-size 16mb \
  --partition-table devices/c606/partitions.csv --target-app-partition ota_1 \
  "$elf" "$image_dir/cycling.bin"
# Keep the matching ELF beside the image: crash decoding needs its symbols.
cp "$elf" "$image_dir/cycling.elf"
# And one per commit and variant, named as the panic record's elf= says, in
# the primary checkout's .local so every worktree's builds are found
# (mise run backtrace). Written under a temporary name, then moved, so
# parallel builds never leave a torn file; the newest 24 are kept.
variant="sdk${CYCLING_SDK:-0}-h${CYCLING_HARNESS:-1}"
if [[ "${CYCLING_BULK_MAINTENANCE:-0}" == 1 ]]; then
  variant="$variant-maint"
fi
suffix=""
if [[ "$CYCLING_BUILD_DIRTY" == 1 ]]; then
  suffix="-dirty"
fi
archive="$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")/.local/elf"
mkdir -p "$archive"
cp "$elf" "$archive/.$CYCLING_BUILD_COMMIT$suffix-$variant.elf.$$"
mv -f "$archive/.$CYCLING_BUILD_COMMIT$suffix-$variant.elf.$$" "$archive/$CYCLING_BUILD_COMMIT$suffix-$variant.elf"
# Best effort: a concurrent build in another worktree may prune a file between
# the glob and ls, which must not fail this build.
{ ls -1t "$archive"/*.elf 2>/dev/null | tail -n +25 | xargs -r rm -f; } || true
