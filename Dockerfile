# syntax=docker/dockerfile:1
FROM lukemathwalker/cargo-chef:latest-rust-1.71.1-bullseye as chef
WORKDIR /app
RUN apt update && apt install lld clang -y

FROM chef as planner
COPY . .
# Compute a lock-like file for our project
RUN cargo chef prepare  --recipe-path recipe.json

FROM debian:bullseye as builder
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

# Install Rust
RUN curl https://sh.rustup.rs -sSf | bash -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

WORKDIR /app
COPY --from=planner /app/recipe.json recipe.json
RUN cargo install cargo-chef bunyan
# Build our project dependencies, not our application!
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
# Build our project
RUN cargo update && cargo build --release

FROM debian:bullseye-slim as runtime
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update
RUN apt-get install -y libgstreamer1.0-0 \
    gstreamer1.0-plugins-base \
    gstreamer1.0-plugins-good \
    gstreamer1.0-plugins-bad \
    gstreamer1.0-plugins-ugly \
    gstreamer1.0-libav \
    gstreamer1.0-tools \
    gstreamer1.0-nice

WORKDIR /app
COPY --from=builder /app/target/release/srt-whep ./srt-whep

ENV GST_DEBUG=1
ENTRYPOINT [ "./srt-whep" ]
