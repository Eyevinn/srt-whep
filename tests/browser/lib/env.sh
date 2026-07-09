#!/bin/sh
# GStreamer framework env for building/running srt-whep on macOS.
# Keep GST_PLUGIN_PATH on the framework ONLY — do not prepend a separately-built
# gst-plugins-rs, or a higher-versioned rswebrtc shadows the framework's matched
# one and breaks the WebRTC media path (see docs/adr/0003).
GST=/Library/Frameworks/GStreamer.framework/Versions/Current
export PATH="$GST/bin:$PATH"
export PKG_CONFIG_PATH="$GST/lib/pkgconfig"
export GST_PLUGIN_PATH="$GST/lib"
export DYLD_FALLBACK_LIBRARY_PATH="$GST/lib"
export DYLD_LIBRARY_PATH="${DYLD_LIBRARY_PATH:+$DYLD_LIBRARY_PATH:}$GST/lib"
