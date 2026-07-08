# 4. Docker runtime image sourced from a base that bundles `rswebrtc`

**Status:** Accepted, 2026-07-08

## Context

[ADR 0003](0003-webrtc-plugin-from-installation.md) stopped compiling the WHIP
sink into the binary: srt-whep now loads `whipclientsink` from whichever
`rswebrtc` plugin the GStreamer *installation* provides. On macOS the framework
supplies it, and that fix was verified end-to-end.

That change moved the problem on Linux. The old Docker image installed the
stock `gstreamer1.0-plugins-{base,good,bad,ugly,libav}` set and relied on the
compiled-in `plugin_register_static()` copy for `whipclientsink`. With the
static copy gone, the runtime needs the system to provide the element — and
**no Debian or Ubuntu release packages the `rswebrtc` / `webrtchttp` plugin.**
Verified directly:

- Debian bookworm, trixie, testing, sid: no `gstreamer1.0-plugins-rs` package;
  only Rust FFI `-dev` binding source packages exist.
- Fedora: only `rust-gst-plugin-*-devel` *source* crates, no runtime plugin,
  and no webrtc crate at all.
- `restreamio/gstreamer` (1.20.2 and 1.28.4 tags): no `whipclientsink`.

So the runtime image (bookworm, GStreamer 1.22) shipped **no source of
`whipclientsink` at all** — it would start, ingest SRT, and then fail every
viewer branch at `ElementFactory::make("whipclientsink")`. `gst-plugins-rs` is
effectively only available prebuilt in images where someone compiled it.

## Decision

**Base the runtime stage on `livekit/gstreamer:1.26.7-prod-rs`, which bundles
`gst-plugins-rs` (`libgstrswebrtc.so` → `whipclientsink`) in the default plugin
path, together with the SRT, `tsdemux` and RTP elements the pipeline needs.**

- Runtime stage: `FROM livekit/gstreamer:1.26.7-prod-rs`. The plugin is on the
  default registry scan path, so no `GST_PLUGIN_PATH` is set.
- Builder stage: **unchanged** — still the `cargo-chef` bookworm image building
  against GStreamer 1.22. GStreamer keeps a stable ABI across the 1.x series, so
  a binary built against 1.22 runs on the 1.26 runtime. Per ADR 0003 the binary
  is version-agnostic about `rswebrtc` (it resolves the element by name at
  runtime), so only the core GStreamer/glib ABI has to match, and older-built →
  newer-runtime is the safe direction. glibc likewise (bookworm 2.36 → Ubuntu
  24.04 2.39).
- The main binary needs only glibc, glib and the GStreamer libraries at runtime
  (`reqwest`/openssl are dev-dependencies only), all present in the base.

Alternatives considered and rejected: compiling `gst-plugins-rs` from source in
the builder (heavy build, version-pinning burden), and re-introducing a guarded
compiled-in fallback (a Rust change that re-adds the dependency ADR 0003
removed). Switching to a base that already ships the plugin keeps the binary
clean and the Dockerfile change small.

## Consequences

- The Docker image gains a working WHEP media path and tracks the base image's
  GStreamer version (1.26.7) rather than Debian stable's (1.22). The image is
  larger than the previous `slim-bookworm` runtime.
- The image now depends on the `livekit/gstreamer` base staying available and
  continuing to ship `-prod-rs` tags. Bumping GStreamer is a matter of moving
  the base tag; the builder need not match it exactly, only stay older-or-equal.
- **This fixes only the Docker image.** A plain `apt install` of GStreamer on
  Debian/Ubuntu — i.e. the build-from-source and `cargo install srt_whep`
  paths — still has no `whipclientsink`. Those users must supply an `rswebrtc`
  plugin themselves (build `gst-plugins-rs`, or run the Docker image). This is
  called out in [`docs/known_limitations.md`](../known_limitations.md).
- CI is unaffected in principle: the PR workflow only *builds* the image (it
  never runs it), and the test job's real-pipeline `e2e_gstreamer` test is
  `#[ignore]`d, so `cargo test --all-targets` compiles but never instantiates a
  real `whipclientsink`.
