# syntax=docker/dockerfile:1
FROM lukemathwalker/cargo-chef:latest-rust-bookworm AS chef
WORKDIR /app
RUN apt update && apt install lld clang -y

FROM chef AS planner
COPY . .
# Compute a lock-like file for our project
RUN cargo chef prepare  --recipe-path recipe.json

FROM chef AS builder
ENV DEBIAN_FRONTEND=noninteractive

# Update default packages
RUN apt-get update

# Get Ubuntu packages
RUN apt-get install -y \
    build-essential \
    curl \
    pkg-config

# Get GStreamer-related packages
RUN apt-get install -y libssl-dev \
    libunwind-dev \
    libgstreamer1.0-dev \
    libgstreamer-plugins-base1.0-dev \
    libgstreamer-plugins-bad1.0-dev \
    gstreamer1.0-plugins-base \
    gstreamer1.0-plugins-good \
    gstreamer1.0-plugins-bad \
    gstreamer1.0-plugins-ugly \
    gstreamer1.0-libav \
    gstreamer1.0-tools \
    gstreamer1.0-x \
    gstreamer1.0-alsa \
    gstreamer1.0-gl \
    gstreamer1.0-gtk3 \
    gstreamer1.0-qt5 \
    gstreamer1.0-pulseaudio \
    gstreamer1.0-nice

WORKDIR /app
COPY --from=planner /app/recipe.json recipe.json
# Build our project dependencies, not our application!
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
# Build our application
RUN cargo build --release

# Runtime image.
#
# srt-whep no longer compiles in its own WebRTC sink; it loads `whipclientsink`
# from whichever `rswebrtc` plugin the GStreamer installation provides (see
# docs/adr/0003 and docs/adr/0004). No Debian/Ubuntu apt package ships that
# plugin, so the plain gstreamer1.0-plugins-* set used before this base leaves
# `whipclientsink` missing and every viewer branch fails. livekit/gstreamer's
# `-prod-rs` image bundles gst-plugins-rs (libgstrswebrtc.so) in the default
# plugin path, alongside the SRT, tsdemux and RTP elements the pipeline needs.
#
# The builder above compiles against GStreamer 1.22 (bookworm); GStreamer keeps
# a stable ABI across the 1.x series, so that binary runs on this newer 1.26
# runtime. The binary itself is version-agnostic about rswebrtc — it resolves
# the element by name at runtime — so only the core GStreamer/glib ABI matters.
FROM livekit/gstreamer:1.26.7-prod-rs AS runtime
WORKDIR /app
COPY --from=builder /app/target/release/srt-whep ./srt-whep

ENV GST_DEBUG=1
ENTRYPOINT [ "./srt-whep" ]
