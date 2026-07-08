# 3. WHIP sink from the GStreamer installation, not a statically-registered crate

**Status:** Accepted, 2026-07-08

## Context

The WHEP output is produced by a `whipclientsink` — gst-plugin-webrtc's
`WhipWebRTCSink`, hot-plugged into the pipeline per viewer (see the **Branch**
term in [`CONTEXT.md`](../../CONTEXT.md)). Historically this app bound that
plugin two ways at once: it depended on the `gst-plugin-webrtc` /
`gst-plugin-rtp` crates and called `gstrswebrtc::plugin_register_static()` at
startup to register a copy **compiled into the binary**, while also relying on
the GStreamer installation to provide the same elements at runtime.

Those two copies drift. The crates were pinned to a gstreamer-rs release from
the GStreamer 1.24 era (`gst-plugin-webrtc` 0.13.x); the installed GStreamer
had moved to 1.28.4, whose bundled `rswebrtc` plugin is 0.15.2. Because
`plugin_register_static()` runs first, the crate-pinned 0.13.5 elements won the
plugin registry and **shadowed** the installed 0.15.2. A WebRTC sink built for
1.24 then drove 1.28.4's C elements: `rtph264pay`'s caps handshake
(`gst_rtp_h264_pay_setcaps` → `set_sps_pps`) failed → `not-negotiated` on the
video app source → the whole WHIP session aborted with "Internal data stream
error". One `webrtcbin` carries both tracks, so the video failure took audio
down with it — viewers connected (ICE completed) but received **no media at
all**.

## Decision

**Use whatever `rswebrtc` the GStreamer installation provides; do not compile
in or statically register a second copy.**

- Dropped `gstrswebrtc::plugin_register_static()` from pipeline init
  (`src/stream/gst_pipeline.rs`). `whipclientsink` now resolves from the
  installed plugin path — on macOS the framework's own
  `libgstrswebrtc.dylib`, matched to the framework's C elements.
- Set the WHIP signaller's `whip-endpoint` through a plain GObject property
  (`whipsink.property::<gst::glib::Object>("signaller")` →
  `set_property_from_str`) in `src/stream/branch.rs`, instead of the
  `WhipWebRTCSink` / `Signallable` Rust types. No version-locked webrtcsink
  type is compiled into the binary anymore.
- Removed `gst-plugin-webrtc` and `gst-plugin-rtp` from `Cargo.toml`
  (`gst-plugin-rtp` had no code usage at all).
- Kept `gstreamer` / `gstreamer-pbutils` at 0.23.x: those bindings are
  forward-compatible with the newer runtime for the operations this app uses,
  so a full gstreamer-rs binding migration was **not** required to fix the
  media path.

## Consequences

- The binary trusts the GStreamer installation to provide a `whipclientsink`
  that matches its own C elements — the pairing that actually works. The
  installation must therefore include the `rswebrtc` plugin; recent GStreamer
  builds bundle it.
- **Runtime plugin selection now matters.** If a second `rswebrtc` is on
  `GST_PLUGIN_PATH` (e.g. a locally built gst-plugins-rs) and is a higher
  version, it wins by version and is used instead. Run with the
  installation's plugin path only, unless you deliberately intend to override
  it. On macOS this means `GST_PLUGIN_PATH` should point at
  `…/GStreamer.framework/Versions/Current/lib` and **not** prepend a separate
  build.
- srt-whep no longer has a compile-time handle on the webrtcsink type; the
  `signaller` object and its `whip-endpoint` property are treated as the
  element's stable API.
