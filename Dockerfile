# syntax=docker/dockerfile:1
FROM debian:bullseye
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

WORKDIR /src
ADD ./ /src
RUN cargo update && cargo build --release

FROM debian:bullseye
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
COPY --from=0 /src/target/release/srt-whep ./srt-whep

ENV GST_DEBUG=1
ENTRYPOINT [ "./srt-whep" ]
