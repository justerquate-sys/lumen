#!/bin/sh
# Builds the TV app: the shared client's TV bundle (./build.sh --tv writes tv/shell/src-tauri/bundle,
# the APK's built-in copy; extension/ is not touched), then the APK.
#   tv/scripts/build.sh            debug APK, arm64 (emulator, quick checks)
#   tv/scripts/build.sh --release  release APK, arm64 + armv7 (Chromecast with Google TV and many
#                                  TVs run 32-bit Android), signed with the release keystore
# Options: --no-client (reuse the last bundle), SEQ=<n> (bundle sequence; default: now).
# The client step builds the public bundle and its development variant (content-dev.js, speed telemetry:
# run only while the owner turned telemetry on); the public build runs last, so target/pkg stays public.
# Output: tv/out/lumen-tv-<version>[-debug].apk
set -eu
here="$(cd "$(dirname "$0")" && pwd)"; tv="$(dirname "$here")"; root="$(dirname "$tv")"
. "$here/env.sh"
mode=debug; client=1
for a in "$@"; do case "$a" in --release) mode=release;; --no-client) client=0;; *) echo "unknown option $a" >&2; exit 2;; esac; done
if [ "$client" = 1 ]; then "$root/build.sh" --tv --dev; "$root/build.sh" --tv; fi
for f in content.js client.css native.css rules.json seq client; do
  [ -f "$tv/shell/src-tauri/bundle/$f" ] || { echo "no client bundle: run without --no-client" >&2; exit 1; }
done
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$tv/shell/src-tauri/Cargo.toml" | head -1)"
mkdir -p "$tv/out"
cd "$tv/shell/src-tauri"
if [ "$mode" = release ]; then
  cargo tauri android build --apk --target aarch64 --target armv7 --config "{\"version\":\"$version\"}"
  apk="$(ls -t gen/android/app/build/outputs/apk/universal/release/*.apk | head -1)"
  cp "$apk" "$tv/out/lumen-tv-$version.apk"; echo "$tv/out/lumen-tv-$version.apk"
else
  cargo tauri android build --debug --apk --target aarch64 --config "{\"version\":\"$version\"}"
  apk="$(ls -t gen/android/app/build/outputs/apk/universal/debug/*.apk | head -1)"
  cp "$apk" "$tv/out/lumen-tv-$version-debug.apk"; echo "$tv/out/lumen-tv-$version-debug.apk"
fi
