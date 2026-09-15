#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
if [[ ! -f .local/export-esp.sh ]]; then
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
source .local/export-esp.sh
CYCLING_BUILD_COMMIT="$(git rev-parse --short=12 HEAD)"
export CYCLING_BUILD_COMMIT
if [[ -n "$(git status --porcelain --untracked-files=no)" ]]; then
  export CYCLING_BUILD_DIRTY=1
else
  export CYCLING_BUILD_DIRTY=0
fi
export CARGO_TARGET_DIR="$PWD/$firmware_target"
cargo +cycling-esp build -p c606-firmware --bin cycling-os --release --locked --no-default-features --features "$firmware_features" --target xtensa-esp32s3-none-elf
espflash save-image --chip esp32s3 --flash-mode dio --flash-freq 80mhz --flash-size 16mb \
  --partition-table devices/c606/partitions.csv --target-app-partition ota_1 \
  "$CARGO_TARGET_DIR/xtensa-esp32s3-none-elf/release/cycling-os" .local/cycling.bin
