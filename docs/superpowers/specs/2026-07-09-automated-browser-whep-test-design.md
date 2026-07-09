# Automated browser-driven WHEP connection test — design

- **Status**: approved (design), ready for implementation plan
- **Date**: 2026-07-09
- **Author**: brainstormed with Claude

## Problem

srt-whep serves WebRTC/WHEP to browsers. Today the only way to prove a viewer
actually **gets media** is manual: a human runs the `run-srt-whep-codec-test`
skill, opens the diagnostic player (`player/index.html`) in Chrome, clicks
**▶ Connect**, and reads the on-screen "stages" (Connection / ICE / Video
track / Frames decoded / …).

Two existing test layers do **not** cover this:

- `tests/e2e_gstreamer.rs` deliberately **abandons** the WebRTC handshake — it
  drives a real `whipclientsink` only to offer receipt, because feeding a canned
  SDP answer would trigger DTLS/ICE against a nonexistent peer and error the
  branch. Its own docstring says "Media playout is verified manually with the
  WHEP player."
- `tests/signaling.rs` exercises the signaling state machine with fakes, not a
  real browser or real media.

So there is a real gap: **no automated check that a real browser negotiates the
codec and decodes frames.** This is exactly the failure class that bit us before
— WHEP connects (`connectionState: connected`) but zero media flows because of a
GStreamer/plugin version skew (`rtph264pay set_sps_pps` fails and aborts the
session). A "connected but frames == 0" guard would have caught it.

## Goal

A **first-class, repo-level, one-shot** command that:

1. Brings up an SRT source + srt-whep + the diagnostic player.
2. Drives the **real installed Google Chrome** through the full WHEP handshake,
   "as if a human clicked Connect."
3. Checks the connection **stages** and decides pass/fail.
4. Emits **both** a human-readable per-stage report **and** structured JSON.
5. Exits `0` (all gates pass) / `1` (any gate fails or times out).
6. Tears everything down, always.

Non-goals (for this pass): cross-browser (Chrome only; Safari/Firefox stay
manual), CI wiring (build it CI-*ready* but add no workflow), transcoding paths.

## Decisions (locked during brainstorming)

| Decision | Choice | Why |
|---|---|---|
| Output | Both pass/fail exit code **and** detailed report | CI/agent gate + human debugging in one run |
| Orchestration | Full one-shot (bring up → drive → teardown) | Single command; no manual pre-setup |
| Tool | Puppeteer (Node), **Chrome only** | Matches "Chrome by default"; lightest |
| Browser binary | **Real system Google Chrome** (`channel: 'chrome'`), not bundled Chromium | Chromium builds often lack the proprietary H.264 decoder → false `frames == 0`. srt-whep outputs H.264. |
| Location | Repo-level `tests/browser/` | First-class project test, beside `e2e_gstreamer.rs` |
| Skill dedup | **Move** shared scripts/player to the repo, **repoint** the skill | Single source of truth; no drift |
| CI | Local only; leave a seam | macOS + GStreamer framework + real SRT + Chrome is heavy; CI/Docker already finicky |

## Architecture

Clean split of concerns: **shell owns process lifecycle, Node owns the
browser.** The Node driver knows nothing about SRT/GStreamer; the shell knows
nothing about WebRTC.

```
tests/browser/
  README.md              # how to run, prerequisites, reading the output
  package.json           # pins puppeteer (Chrome-only); node_modules/ gitignored
  run.sh                 # ONE-SHOT orchestrator (bash)
  drive-chrome.mjs       # Puppeteer driver (Node)
  lib/
    env.sh               # macOS GStreamer framework env (framework-only GST_PLUGIN_PATH)
    bringup.sh           # SRT source + srt-whep; retries the tsdemux link race
    source-x264.sh       # x264 test source (H.264 profile arg)
  player/
    index.html           # diagnostic WHEP player — single source of truth for the handshake
```

`cargo` ignores `tests/browser/` because it contains no `*.rs` files, so this
does not affect the Rust build or `cargo test`.

### Component: `run.sh` (one-shot orchestrator)

```
run.sh [--profile <p>] [--headed] [--timeout <sec>]   # media-wait default: 5s
       [--endpoint <url>] [--skip-bringup] [--port <http>]

  1. source lib/env.sh                          # macOS GStreamer framework env
  2. ensure node_modules (npm install if missing)
  3. lib/bringup.sh <profile>                   # source + srt-whep, retry tsdemux race
  4. serve player/ on 127.0.0.1:<player-port>   # background http server
  5. node drive-chrome.mjs --endpoint … --timeout … [--headed]
  6. capture the driver's exit code
  → trap 'teardown' EXIT   # kill srt-whep, gst, ffmpeg, http server — ALWAYS
  → exit with the driver's code
```

- `--skip-bringup` + `--endpoint` lets it run against an already-running server
  (skips steps 3–4), for fast iteration — but the default is the full one-shot.
- `trap … EXIT` guarantees teardown on success, failure, or Ctrl-C.
- Reuses the promoted `bringup.sh` retry loop so the known tsdemux link race
  (known_limitations #6) does not flake the test.

### Component: `drive-chrome.mjs` (browser driver)

Owns the browser and only the browser.

```
launch real Chrome (channel: 'chrome', headless: 'new' unless --headed,
       args to allow autoplay / insecure localhost as needed)
open the player page
set the endpoint input, click "▶ Connect"
poll the page via waitForFunction until frames climb OR timeout
scrape the stages from the DOM
compute the verdict
write JSON + print the human report
process.exit(verdict.pass ? 0 : 1)
```

**Why reuse the existing player instead of a dedicated harness page:** the
player already runs the exact handshake a human uses (POST empty body → receive
offer → `setRemoteDescription` → `createAnswer`/`setLocalDescription` → gather
ICE → PATCH answer) and already writes every value we need to stable-id DOM
nodes. A separate headless page would duplicate the handshake and drift from
what humans actually run. Keeping one player = one source of truth. **The player
needs no changes** — the driver reads its existing DOM.

## Data flow: stages checked and gate assertions

The player writes these to stable DOM ids; the driver reads them:

| Stage | DOM source | Gate assertion (pass condition) |
|---|---|---|
| Server offer | `#log` HTTP status line | received, not `503`/error body |
| Connection | `#pc` badge text | `=== "connected"` |
| ICE | `#ice` badge text | `connected` or `completed` (reported; see note) |
| Video not rejected | `#log` | no "ANSWER REJECTED VIDEO (m=video port 0)" line |
| **Frames decoded** | `#frames` text | **`> 0` AND increased between two polls** |
| Bytes recv (video) | `#bytes` ("v / a") | video bytes `> 0` |
| Decoded codec / fmtp | `#codec` | reported (captures `profile-level-id`) |
| Frame size | `#size` | reported (e.g. `320×240`) |

The **frames-increasing** check is the crux: it distinguishes real continuous
playout from a single stuck frame, and it is precisely what catches
"connected but zero media." The driver polls `#frames` at least twice over the
media-wait window (default **5s**; `--timeout`) and requires a strictly
increasing value. 5s at 25fps is ~125 frames — ample to confirm climbing while
keeping runs snappy. This window is the wait for frames *after* Connect; the
slow/racy pipeline start is handled separately by `bringup.sh`'s retry loop.

Note on ICE: srt-whep does SDP-focused, host-candidate signaling (no TURN/ICE
negotiation, per the README). On localhost host candidates connect reliably. ICE
state is reported in the output but the hard media gate is `connectionState ==
connected` + frames increasing, which is the meaningful success signal.

## Output

1. **Human report** to stdout — a per-stage table (✓/✗ per gate), decoded codec
   + `profile-level-id`, frames / bytes / frame size, and the offer/answer video
   m-lines. Mirrors what the manual diagnostic panel shows.
2. **Structured JSON** to `target/codec-test/whep-auto-<profile>.json` (the
   existing gitignored codec-test output dir), e.g.:
   ```json
   {
     "pass": true,
     "profile": "constrained-baseline",
     "stages": {
       "offer": {"ok": true, "status": 201},
       "connection": {"ok": true, "value": "connected"},
       "videoRejected": {"ok": true},
       "frames": {"ok": true, "first": 12, "last": 187, "increasing": true},
       "videoBytes": {"ok": true, "value": 240113},
       "codec": {"value": "video/H264", "fmtp": "…profile-level-id=42e01f…"},
       "frameSize": {"value": "320x240"}
     },
     "log": ["…full player log…"]
   }
   ```
3. **Exit code** `0` if every gate passes, `1` otherwise (fail or timeout). The
   failing stage is named in both the report and JSON.

## Reliability

Inherits the codec-test skill's hard-won mitigations by reusing its scripts:

- **tsdemux link race** — `bringup.sh` retries the whole bring-up until the
  pipeline links cleanly (known_limitations #6).
- **VideoToolbox exhaustion** — default source is x264 (`source-x264.sh`), which
  does not hold VT sessions. (VT/file sources are out of scope for this pass;
  the seam is a `--source` passthrough later.)
- **Mixed content** — the bundled same-origin `http://` player avoids the
  HTTPS-player mixed-content failure mode; Chrome connects to `http://localhost`
  fine.
- **Teardown** — `trap … EXIT` in `run.sh` prevents orphaned srt-whep / gst /
  ffmpeg / http-server processes across runs.

## Footprint

- One dev dependency: `puppeteer`, isolated in `tests/browser/`. `node_modules/`
  added to `.gitignore`. The Rust crate, `Cargo.*`, and `cargo test` are
  untouched.
- First `run.sh` invocation runs `npm install` if `node_modules` is missing.

## Skill change (single source of truth)

`.claude/skills/run-srt-whep-codec-test/` currently owns its own copies of
`env.sh`, `bringup.sh`, `source-x264.sh`, and `player/index.html`. These move to
`tests/browser/` (and `tests/browser/lib/`). The skill's `SKILL.md` and its
remaining scripts are updated to reference the promoted repo copies so there is
one source of truth and no drift. The skill keeps its manual/interactive value
(open the player in Chrome AND Safari by hand) and gains the automated path via
`tests/browser/run.sh`. Skill-only scripts not needed by the automated test
(e.g. `source-vt.sh`, `source-file.sh`, `server.sh`) may stay in the skill or
also move — resolved in the plan.

## Verifying the tool itself

Before declaring done, prove it discriminates on macOS:

- **Positive**: full `run.sh` against a good x264 stream → exit `0`, `#frames`
  climbing, correct `profile-level-id` in the report.
- **Negative**: point at a stopped source / wrong endpoint → exit `1` with the
  correct failing stage named (e.g. `connection` or `frames`), confirming it
  actually catches the "connected but no media" / no-connection cases it exists
  to guard.

## Open items for the implementation plan

- Exact HTTP server for the player (reuse `scripts/test_server.py` vs a
  one-liner `python3 -m http.server`) and its port.
- Which skill scripts move vs. stay.
- Puppeteer launch args needed for headless WebRTC video decode on macOS.
- Whether to add a tiny `window.__whepState()` hook to the player later for more
  robust reads (deferred; DOM scraping is sufficient and touches nothing).
```
