#!/bin/sh
# From expo/exact docs/reference.md, "Native CI preparation". Prints a TIMING line per phase.
set -eu
t() { echo "TIMING prepare.$1 $(( $(date +%s) - $2 ))s"; }
export EXACT="$PWD/../exact"
s=$(date +%s); git clone https://github.com/expo/exact.git "$EXACT"; git -C "$EXACT" checkout --detach "$EXACT_REF"; t clone $s
s=$(date +%s); bun install --frozen-lockfile --cwd "$EXACT"; t bun $s
if ! command -v rustup >/dev/null; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/exact-rustup.sh
  sh /tmp/exact-rustup.sh -y --profile minimal --default-toolchain none --no-modify-path
fi
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
pin=$(bun -e 'console.log(Bun.TOML.parse(await Bun.file(process.env.EXACT + "/rust-toolchain.toml").text()).toolchain.channel)')
case "$1" in
  ios) triple=aarch64-apple-ios ;;
  android) triple=aarch64-linux-android ;;
  *) exit 2 ;;
esac
s=$(date +%s); rustup toolchain install "$pin" --profile minimal --target "$triple"; t rustup $s
s=$(date +%s); cargo +"$pin" run --locked --manifest-path "$EXACT/vendor/ibex/crates/hermes-lean-sys-installer/Cargo.toml" -- --target "$triple"; t hermes $s
s=$(date +%s); cargo +"$pin" fetch --locked --manifest-path "$EXACT/Cargo.toml"; t fetch $s
set-env EXACT "$EXACT"
set-env PATH "$PATH"
