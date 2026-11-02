# YourTube debug center (optional developer tool)

A local collector for YourTube **development builds** (`./build.sh --dev`). Rust, standard library only.
It listens on `127.0.0.1:47321` only and appends every report the extension's background worker POSTs to
`/incident` (incidents and speed-telemetry batches) as one line to its incidents file. The public build
has no collector code and never talks to it.

| Route | What |
|---|---|
| `POST /incident` | Append a report (≤ 512 KB): `{"received": <epoch ms>, "report": <the JSON>}` per line. |
| `GET /` | The latest 30 incidents (telemetry batches are only counted). |
| `GET /perf?since=<7d>&version=<build>&metric=<prefix*>` | Speed dashboard: per metric count/p50/p90/p99/max, p50 trend per build, incidents per build, the 20 slowest events per metric with their breakdown. |
| `GET /perf.json?metric=<name or prefix*>&since=<epoch ms, ms ago, 30m/6h/7d, all>&version=<build>&limit=<n>` | Raw telemetry rows (JSON array, oldest first): the event plus `received`, `version`, `page`. |
| `GET /health` | `ok` |

The file is streamed for every view (lines received before the window are skipped unparsed); each
connection runs on its own thread. 50,000 lines (69 MB, ~550,000 events) render in ~0.35 s; a 24-hour
window of them in ~0.03 s. What the reports and events mean: [docs/debug-center.md](../../docs/debug-center.md).

## Where the incidents file lives

The first argument names the file. Without one it goes to your user data folder, never into the
repository:

| OS | Default incidents file |
|---|---|
| macOS | `~/Library/Application Support/YourTube/incidents.ndjson` |
| Linux | `$XDG_DATA_HOME/yourtube/incidents.ndjson` (usually `~/.local/share/yourtube/incidents.ndjson`) |
| Windows | `%APPDATA%\YourTube\incidents.ndjson` |

Reports contain no cookies, tokens, search terms or keys, but they do contain video ids, timings and your
browser's user agent: keep the file to yourself.

## Run it

```sh
cd tools/debug-center
cargo build --release --locked && cargo test --locked
./target/release/yourtube-debug-center            # default incidents file (above)
./target/release/yourtube-debug-center /tmp/x.ndjson
```

Then open http://127.0.0.1:47321/ and http://127.0.0.1:47321/perf.
`YTC_DEBUG_CENTER_ADDR=127.0.0.1:<port>` runs a second instance on another port (tests); the extension
itself only talks to port 47321 (or to the URL in its `ytcCollectorV1` storage key).

## Start it at login (macOS)

Copy the binary somewhere stable and add a LaunchAgent, e.g.
`~/Library/LaunchAgents/com.yourtube.debug-center.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.yourtube.debug-center</string>
  <key>ProgramArguments</key><array>
    <string>/Users/YOU/Library/Application Support/YourTube/yourtube-debug-center</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardErrorPath</key><string>/Users/YOU/Library/Application Support/YourTube/debug-center.log</string>
</dict></plist>
```

With no second `ProgramArguments` entry the incidents file is the default one above. Load it with
`launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.yourtube.debug-center.plist`, restart it
after replacing the binary with `launchctl kickstart -k gui/$(id -u)/com.yourtube.debug-center`, stop it
with `launchctl bootout gui/$(id -u)/com.yourtube.debug-center`.

On Linux a systemd user service (`ExecStart=/path/to/yourtube-debug-center`) does the same; on Windows a
Task Scheduler task "At log on".
