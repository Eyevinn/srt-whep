# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [2.0.0] - 2026-07-08

A ground-up rewrite of the signaling and streaming internals, plus fixes that
restore WebRTC media delivery on both Linux and macOS. Same product — ingest one
MPEG-TS-over-SRT stream and fan it out to WHEP/WebRTC viewers — on a far more
robust core. Existing CLI invocations keep working: every new flag defaults to
the prior behavior, so no configuration migration is required.

### Added

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

[2.0.0]: https://github.com/Eyevinn/srt-whep/releases/tag/v2.0.0
