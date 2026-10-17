#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
# Linked worktrees use the toolchain export of the primary checkout.
export_esp=".local/export-esp.sh"
if [[ ! -f "$export_esp" ]]; then
  export_esp="$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")/.local/export-esp.sh"
fi
if [[ ! -f "$export_esp" ]]; then
  echo "Run mise run setup first." >&2
  exit 1
fi
case "${CYCLING_HARNESS:-1}" in
  1) firmware_features="hardware,debug-harness"; firmware_target=".local/firmware" ;;
  0) firmware_features="hardware"; firmware_target=".local/firmware-no-harness" ;;
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
mkdir -p .local
source "$export_esp"
CYCLING_BUILD_COMMIT="$(git rev-parse --short=12 HEAD)"
export CYCLING_BUILD_COMMIT
if [[ -n "$(git status --porcelain --untracked-files=no)" ]]; then
  export CYCLING_BUILD_DIRTY=1
else
  export CYCLING_BUILD_DIRTY=0
fi
export CARGO_TARGET_DIR="$PWD/$firmware_target"
cargo +cycling-esp build -p c606-firmware --bin cycling-os --release --locked --no-default-features --features "$firmware_features" --target xtensa-esp32s3-none-elf
# The main stack is the internal DRAM that statics and the heap leave. Each
# task and VANA's frame preparation run on it, so growing statics silently
# shrinks it until boot overflows the stack guard (#167). Fail the build
# first: keep new state out of internal RAM, or measure STATUS
# stack_untouched on the device and lower the budget deliberately.
min_stack_bytes="${CYCLING_MIN_STACK_BYTES:-61440}"
stack_symbols="$(xtensa-esp32s3-elf-nm "$CARGO_TARGET_DIR/xtensa-esp32s3-none-elf/release/cycling-os")"
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
  "$CARGO_TARGET_DIR/xtensa-esp32s3-none-elf/release/cycling-os" .local/cycling.bin
