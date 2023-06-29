# syntax=docker/dockerfile:1
FROM debian:bookworm
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update
RUN apt-get -y install pkg-config \
  libssl-dev \
  libunwind-dev \
  libgstreamer1.0-dev \
  gstreamer1.0-plugins-base \
  libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-bad \
  gstreamer1.0-plugins-ugly \
  gstreamer1.0-libav \
  libgstrtspserver-1.0-dev \
  libges-1.0-dev

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

FROM debian:bookworm
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update
RUN apt-get -y install pkg-config \
  libssl-dev \
  libunwind-dev \
  libgstreamer1.0-dev \
  gstreamer1.0-plugins-base \
  libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-bad \
  gstreamer1.0-plugins-ugly \
  gstreamer1.0-libav \
  libgstrtspserver-1.0-dev \
  libges-1.0-dev

WORKDIR /app
COPY --from=0 /src/target/release/srt-whep ./srt-whep

ENTRYPOINT [ "./srt-whep" ]
