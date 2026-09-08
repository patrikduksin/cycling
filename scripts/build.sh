#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ ! -f .local/export-esp.sh ]]; then
  echo "Run mise run setup first." >&2
  exit 1
fi
source .local/export-esp.sh
cargo +cycling-esp build --release --locked --features c606 --target xtensa-esp32s3-none-elf
espflash save-image --chip esp32s3 --flash-mode dio --flash-freq 80mhz --flash-size 16mb \
  --partition-table packages/stock/magene-c606/partitions.csv --target-app-partition ota_1 \
  target/xtensa-esp32s3-none-elf/release/cycling-os .local/cycling.bin
