# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [2.1.1] - 2026-07-11

API hardening on the DELETE endpoints and a documentation rebuild. No CLI
changes; the only API-visible change is that DELETE on an already-gone
resource now succeeds with `204` instead of failing with `404`.

### Added

- `docs/connection-lifecycle.md`: a newcomer-facing walkthrough of the
  three-leg WHEP handshake, the parked-waiter concept, and the
  route → command → state mapping.

### Changed

- `DELETE /channel/{id}` and `DELETE /whip_sink/{id}` are now idempotent:
  deleting an already-gone or never-existent resource returns `204 No Content`
  instead of `404 Not Found`, so client retries and races with the
  coordinator's reaper no longer surface as errors. A real termination still
  returns `200`, and a transient teardown failure still returns `503` with
  `Retry-After` (#111).
- Split the shared DELETE handler into intent-named handlers with distinct
  tracing spans: `terminate_session` (span `WHEP DELETE`) for a viewer ending
  its playback session, and `remove_whip_sink` (span `WHIP SINK DELETE`) for
  the internal loopback sink teardown. HTTP behavior is unchanged; the two
  routes are now distinguishable in traces and logs (#114).
- Rebuilt the documentation as a kernel (`README.md` for humans, `CONTEXT.md`
  for agents) plus linked layers, with a no-orphan invariant: every retained
  user-facing doc is reachable from the kernel. The macOS `GIO_EXTRA_MODULES`
  notes are folded into the README (#112).

### Removed

- Dead documentation assets and unused SDP fixtures/scripts with no inbound
  references (`docs/{init.svg,running.svg,Example.gif}`,
  `scripts/{AVC-baseline,AVC-high,AVC-main,Safari-offer,gst-commands}`).

## [2.1.0] - 2026-07-10

Internal hardening, new test tooling, and documentation. No CLI or API
changes — every invocation from 2.0.0 keeps working. This release continues the
architecture-deepening pass from 2.0.0 with behaviour-preserving refactors, adds
an automated browser-driven WHEP media check, and refreshes the architecture
documentation.

### Added

- Automated browser-driven WHEP test (`tests/browser/`): a one-shot orchestrator
  drives real Chrome through the WHEP handshake via Puppeteer and asserts that
  decoded video frames climb, guarding against the connected-but-silent version
  skew regression fixed in 2.0.0. Always emits a pass/fail verdict plus JSON,
  even when Chrome can't launch or the player page fails to load. Documented in
  the README Testing section.
- ADR-0005 documenting that watchdog-triggered pipeline restarts are routed
  through the supervisor.
- Regenerated animated architecture diagram
  (`docs/srt-whep-coordinator-actor.gif`) with its editable Excalidraw source and
  the renderer that produces it.

### Changed

- Watchdog restarts now flow through the pipeline supervisor rather than being
  driven from inside the coordinator, and pipeline `quit`/lifecycle control lives
  behind the `PipelineLifecycle` seam (ADR 0005).
- Tests exercise the signal plane through the public `SignalHandle`; the internal
  command enum is now private, with list/reset unified through one request path.
- The stream bus-reap channel is constructor-injected and typed by branch id
  rather than a bare string.
- `add_branch` now cleans up its own half-attached branch on failure, on the
  coordinator's critical path, restoring the teardown bound from ADR 0002.
- Core GStreamer element names are single-sourced from a `stream::naming` module,
  and the coordinator's configuration defaults are single-sourced as constants.
- Folded the shared string-conversion boilerplate on the SDP newtypes behind a
  macro.
- Refreshed `CONTEXT.md` and the shared-lock → coordinator-actor architecture
  narrative for the supervisor-owned restart.

## [2.0.0] - 2026-07-08

A ground-up rewrite of the signaling and streaming internals, plus fixes that
restore WebRTC media delivery on both Linux and macOS. Same product — ingest one
MPEG-TS-over-SRT stream and fan it out to WHEP/WebRTC viewers — on a far more
robust core. Existing CLI invocations keep working: every new flag defaults to
the prior behavior, so no configuration migration is required.

### Added

- `-D`/`--decode-video` flag: insert an `avdec_h264` before `whipclientsink` so
  webrtcsink receives raw video and re-encodes it, working around a
  caps-negotiation bug in webrtcsink 0.15.x on macOS where H264 passthrough
  fails with not-negotiated on `GstAppSrc:video_0`. Off by default.
- CLI flags for coordinator tunables (handshake timeouts, watchdog threshold);
  defaults unchanged.
- Startup assertion that the HTTP listener port matches the pipeline's loopback
  WHIP port, failing fast on misconfiguration.
- Latency option.
- CI: run the test suite, `fmt`, and `clippy` on pull requests, and smoke-test
  the Docker image (WebRTC element present, binary runs) before publishing.
- Dependabot security updates for Cargo dependencies.
- Documentation: `CONTEXT.md` domain glossary and ADRs 0001–0004; a verified
  macOS codec profile table.

### Changed

- Rebuilt the signaling plane around a single coordinator actor (one tokio task)
  that owns all viewer connection state, with per-viewer failure isolation, a
  consecutive-failure watchdog that restarts a wedged pipeline, and a periodic
  sweep that reaps timed-out and abandoned handshakes (ADR 0001).
- Added a pipeline supervisor: a dedicated restart loop runs the GStreamer
  pipeline and recovers from EOS/errors with backoff; a single Ctrl-C cleanly
  stops signaling, the pipeline, and the HTTP server together.
- Architecture-deepening pass: split the `BranchControl` and `PipelineLifecycle`
  seams, kept the pipeline lock private and never held across awaits, moved the
  GLib main loop to a dedicated thread, and typed the pipeline error seam.
- Ports are typed as `u16` end to end.

### Fixed

- WHEP no-media: resolve the `rswebrtc` plugin from the GStreamer installation
  instead of a shadowing static copy, fixing the version skew that produced
  connected-but-silent WHEP sessions; the Docker image sources `rswebrtc` from
  the LiveKit base so Linux WHEP works out of the box (ADR 0003, 0004).
- Branch-error isolation: one viewer's branch failing no longer restarts the
  whole pipeline and drops every other viewer (ADR 0002).
- Hardened the signaling plane against teardown races and test-environment leaks.

[2.1.1]: https://github.com/Eyevinn/srt-whep/releases/tag/v2.1.1
[2.1.0]: https://github.com/Eyevinn/srt-whep/releases/tag/v2.1.0
[2.0.0]: https://github.com/Eyevinn/srt-whep/releases/tag/v2.0.0
