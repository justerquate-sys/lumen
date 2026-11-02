#!/bin/sh
# Debug loop on the emulator/TV attached over adb: build (unless --no-build), install, launch.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"; . "$here/env.sh"
[ "${1:-}" = --no-build ] || "$here/build.sh" --no-client >/dev/null
adb install -r "$(dirname "$here")/out/lumen-tv-$(sed -n 's/^version = "\(.*\)"/\1/p' "$(dirname "$here")/shell/src-tauri/Cargo.toml" | head -1)-debug.apk" | tail -1
adb shell am force-stop app.lumen.tv
adb shell am start -n app.lumen.tv/.MainActivity | tail -1
