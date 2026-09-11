#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ ! -f .local/export-esp.sh ]]; then
  echo "Run mise run setup first." >&2
  exit 1
fi
source .local/export-esp.sh
python scripts/wifi.py generate
export CYCLING_WIFI_CONFIG="$PWD/.local/wifi/config.rs"
export CARGO_TARGET_DIR="$PWD/.local/firmware"
cargo +cycling-esp build --release --locked --features c606 --target xtensa-esp32s3-none-elf
espflash save-image --chip esp32s3 --flash-mode dio --flash-freq 80mhz --flash-size 16mb \
  --partition-table packages/stock/magene-c606/partitions.csv --target-app-partition ota_1 \
  "$CARGO_TARGET_DIR/xtensa-esp32s3-none-elf/release/cycling-os" .local/cycling.bin
