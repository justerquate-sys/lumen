#!/bin/sh
# Builds the loadable extension (extension/) from the Rust crate (src/) and the browser sources
# (browser/).
#   ./build.sh         public build: what is committed and what users load
#   ./build.sh --dev   development build: local debug-center delivery, speed telemetry, auto-reload
# Needs Rust 1.99.1 with the wasm32-unknown-unknown target (rust-toolchain.toml installs both through
# rustup), wasm-bindgen-cli 0.2.130 and Node 22+. On Windows run it from Git Bash or WSL.
set -eu
cd "$(dirname "$0")"
DEV=
case "${1:-}" in
  --dev) DEV=1 ;;
  '') ;;
  *) echo 'usage: ./build.sh [--dev]' >&2; exit 2 ;;
esac
find_tool() { command -v "$1" 2>/dev/null || echo "$HOME/.cargo/bin/$1"; }
CARGO="${CARGO:-$(find_tool cargo)}"
BINDGEN="${WASM_BINDGEN:-$(find_tool wasm-bindgen)}"
if [ "$("$BINDGEN" --version 2>/dev/null)" != 'wasm-bindgen 0.2.130' ]; then
  echo 'This build requires wasm-bindgen-cli 0.2.130 (cargo install wasm-bindgen-cli --version 0.2.130 --locked).' >&2
  exit 1
fi
"$CARGO" build --release --locked --target wasm32-unknown-unknown ${DEV:+--features dev}
"$BINDGEN" target/wasm32-unknown-unknown/release/ytc_client.wasm --target no-modules --remove-name-section --remove-producers-section --out-dir target/pkg --out-name ytc_client
node browser/build.cjs ${DEV:+--dev}
