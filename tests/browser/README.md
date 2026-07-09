# Automated browser-driven WHEP test

Brings up srt-whep + an SRT source, drives the **real system Google Chrome**
through the full WHEP handshake, checks the connection stages, and exits `0`
(all gates pass) / `1` (any fail). Emits a per-stage report and a JSON file.

This is the automated counterpart to the manual `run-srt-whep-codec-test`
skill — same diagnostic player, same bring-up scripts, no clicking.

## Prerequisites (macOS)

- Release binary built: `cargo build --release` (see repo README for the
  GStreamer framework setup).
- GStreamer framework with `rswebrtc` (`gst-inspect-1.0 whipclientsink` works).
- Google Chrome installed at `/Applications/Google Chrome.app` (or set
  `CHROME_PATH`). **Real Chrome, not Chromium** — Chromium may lack the H.264
  decoder and would show 0 frames on a good stream.
- Node 20+ and `npm` on PATH. `ffmpeg` only for VideoToolbox profiles.

## Run

```sh
tests/browser/run.sh                                  # constrained-baseline, 5s media wait
tests/browser/run.sh --profile main --timeout 8       # another profile, longer wait
tests/browser/run.sh --headed                         # watch the Chrome window
tests/browser/run.sh --skip-bringup \
  --endpoint http://localhost:8000/channel            # drive against an already-running server
```

First run does `npm install` (installs `puppeteer-core`; no browser download).

## Reading the output

A per-stage table prints to stdout; full detail (incl. offer/answer video
m-lines and the player log) lands in `target/codec-test/whep-auto-<profile>.json`.

| Stage | Pass condition |
|---|---|
| offer | server returned an SDP offer (not 503/error) |
| connection | `connectionState === connected` |
| video accepted | no `m=video port 0` rejection in the answer |
| **frames decoded** | **strictly increasing and > 0** across the media wait |
| video bytes | `> 0` |

The frames gate is the crux: `connection connected` but `frames 0 → 0` is the
classic "connected but zero media" failure (e.g. a GStreamer/plugin version
skew), and this test fails it.

## Teardown

`run.sh` tears everything down on exit (even on Ctrl-C). If a run is killed
uncleanly:

```sh
pkill -9 -f 'target/release/srt-whep'; pkill -9 -f 'gst-launch-1.0.*srtsink'
pkill -9 -f 'ffmpeg.*mpegts'; pkill -9 -f 'http.server'
```
