#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ ! -f .local/export-esp.sh ]]; then
  echo "Run mise run setup first." >&2
  exit 1
fi
case "${CYCLING_HARNESS:-1}" in
  1) firmware_features="c606,debug-harness"; firmware_target=".local/firmware" ;;
  0) firmware_features="c606"; firmware_target=".local/firmware-no-harness" ;;
  *) echo "CYCLING_HARNESS must be 0 or 1." >&2; exit 1 ;;
esac
echo "Building with USB test harness=${CYCLING_HARNESS:-1}"
source .local/export-esp.sh
python scripts/wifi.py generate
python scripts/ble_config.py
export CYCLING_WIFI_CONFIG="$PWD/.local/wifi/config.rs"
export CYCLING_BLE_CONFIG="$PWD/.local/ble/config.rs"
export CARGO_TARGET_DIR="$PWD/$firmware_target"
cargo +cycling-esp build --release --locked --no-default-features --features "$firmware_features" --target xtensa-esp32s3-none-elf
espflash save-image --chip esp32s3 --flash-mode dio --flash-freq 80mhz --flash-size 16mb \
  --partition-table packages/stock/magene-c606/partitions.csv --target-app-partition ota_1 \
  "$CARGO_TARGET_DIR/xtensa-esp32s3-none-elf/release/cycling-os" .local/cycling.bin
