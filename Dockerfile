# syntax=docker/dockerfile:1
FROM debian:bullseye
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update
RUN apt-get -y install libgstreamer1.0-0 \
  gstreamer1.0-plugins-bad \
  gstreamer1.0-plugins-good \
  gstreamer1.0-libav \
  gstreamer1.0-plugins-rtp \
  gstreamer1.0-nice
RUN apt-get -y install build-essential \
  curl \
  libglib2.0-dev \
  libgstreamer1.0-dev \
  libgstreamer-plugins-bad1.0-dev
RUN curl https://sh.rustup.rs -sSf | bash -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

WORKDIR /src
ADD ./ /src
RUN cargo update && cargo build --release

FROM debian:bullseye
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update
RUN apt-get -y install libgstreamer1.0-0 \
  gstreamer1.0-plugins-bad \
  gstreamer1.0-plugins-good \
  gstreamer1.0-libav \
  gstreamer1.0-plugins-rtp \
  gstreamer1.0-nice
WORKDIR /app
COPY --from=0 /src/target/release/srt-whep ./srt-whep

ENTRYPOINT [ "./srt-whep" ]