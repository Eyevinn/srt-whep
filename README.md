# SRT to WHEP
This application ingests one MPEG-TS over SRT stream and outputs to WebRTC recvonly clients using WHEP as signaling protocol. Example of use cases:

- Browser based confidence monitor of an incoming stream
- Program or preview output monitor in a browser or tablet

Supports SRT streams in caller and listener mode.
Runs on MacOS and Ubuntu.

![screenshot](docs/screenshot.png)

## Install

```
cargo install srt_whep

# recommended for pretty log viewer (optional)
cargo install bunyan
```

Generate an SRT test source for example using our testsrc Docker container:

```
docker run --rm -p 1234:1234/udp eyevinntechnology/testsrc
```

An SRT stream (in listener mode) is then available at `srt://127.0.0.1:1234`. Then run the `srt-whep` application:

```
srt-whep -i 127.0.0.1:1234 -o 127.0.0.1:8888 -p 8000 -s caller | bunyan
```

It will connect to the SRT test stream in caller mode as the generated SRT stream is in listener mode.

WHEP endpoint is available at `http://localhost:8000/channel`. You can then play it for example using the WHEP [Player](https://webrtc.player.eyevinn.technology/?type=whep). Possible issues are discussed in [Issues](#issues).

If you don't have Rust install you can use the Docker Container image published on Docker Hub:

```
docker run --rm --network host eyevinntechnology/srt-whep \
  -i 127.0.0.1:1234 \
  -o 0.0.0.0:8888 \
  -p 8000 -s caller
```

Note that the container needs to run in host-mode.

## Build from Source
### OSX

Requirements:
- XCode command line tools installed
- GStreamer [binaries](https://gstreamer.freedesktop.org/data/pkg/osx/) from GStreamer's website installed
- Rust and cargo installed

Make sure you have the following env variables defined:

```
export PATH=$PATH:/Library/Frameworks/GStreamer.framework/Versions/Current/bin
export PKG_CONFIG_PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/lib/pkgconfig
export GST_PLUGIN_PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/lib
export DYLD_FALLBACK_LIBRARY_PATH=$GST_PLUGIN_PATH
```

Build with Cargo

```
cargo check
cargo install bunyan # Optional, for pretty printing of logs
cargo build --release
```

The binary is then available at `./target/release/srt-whep`. See below for how to run it.

### Debian (bullseye / bookworm)

Requirements:
- Rust and cargo installed

Install GStreamer build dependencies.

```
apt-get update
apt-get -y install build-essential \
  curl \
  pkg-config \
  libssl-dev \
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
```

Build with Cargo

```
cargo check
cargo install bunyan # Optional, for pretty printing of logs
cargo build --release
```

The binary is then available at `./target/release/srt-whep`. See below for how to run it.

## Docker Container

Build container (uses multi-stage builds):

```
docker build -t srt-whep:dev .
```

Container must be running in host-mode (only works on Linux hosts, and is not supported on Docker Desktop for Mac, Docker Desktop for Windows)

```
docker run --rm --network host srt-whep:dev \
  -i <SRT_SOURCE_IP>:<SRT_SOURCE_PORT> \
  -o 0.0.0.0:8888 \
  -p 8000 -s caller
```

## Usage

To ingest an SRT stream with address `srt://127.0.0.1:1234` in listener mode and expose WHEP endpoint on port 8000 run the application with this command.

```
cargo run --release -- -i 127.0.0.1:1234 -o 127.0.0.1:8888 -p 8000 -s caller | bunyan
```

This will also make a pass-through of the SRT stream on `srt://127.0.0.1:8888` in listener mode. To watch the pass-through stream in ffplay, VLC or GStreamer you run:

```
ffplay srt://127.0.0.1:8888
# or
gst-launch-1.0 playbin uri="srt://127.0.0.1:8888"
```

WHEP endpoint is available then at `http://localhost:8000/channel`. You can then play it for example using the WHEP [Player](https://webrtc.player.eyevinn.technology/?type=whep).

If the SRT stream to ingest is in caller mode you run the application with this command.

```
cargo run --release -- -i 127.0.0.1:1234 -o 127.0.0.1:8888 -p 8000 -s listener | bunyan
```

This also expects the SRT address `127.0.0.1:8888` to be running in caller mode.

## Debugging

- If you doubt a plugin is missing you can check it using `gst-inspect-1.0 <plugin>`. For example, `gst-inspect-1.0 srtsink`.
- It's possible to generate a test SRT stream using GStreamer for debugging purpose. Please refer to [macOS](docs/SRT_macOS.md) or [Ubuntu](docs/SRT_Ubuntu.md).

- To get more verbose logging you can set the `GST_DEBUG` environment variable to `2`. For example, run in terminal: `export GST_DEBUG=2`

## Issues
All relevant discussions are tracked in [issues](https://github.com/Eyevinn/srt-whep/issues/). Please feel free to open a new issue if you have any questions or problems.

- For Mac users, please notice that only H264 video of `constrained-baseline` profile is supported by Safari.
- For Ubuntu users, please notice that H264 video of `high` profile is not supported by broswers (`baseline` or `main` is supported). Related discussions can be found [here](https://askubuntu.com/questions/1412934/webrtc-h-264-high-profile-doesnt-want-to-play-in-browser).
- For Ubuntu users, please notice issues related to hostname resolution. It can be dodged by disabling `Anonymize local IPs exposed by WebRTC` on Chrome. Related discussions can be found [here](https://support.ipconfigure.com/hc/en-us/articles/360031237552-WebRTC-not-working-in-Google-Chrome-over-local-network-mDNS-)
- We don't support the client side init mode of WHEP. This is under discussion but as the server knows what streams it has we believe the server should provide the SDP offer. Related discussions can be found [here](docs/whep.md).

## License (Apache-2.0)

Copyright 2023 Eyevinn Technology AB

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.

## Support

Join our [community on Slack](http://slack.streamingtech.se) where you can post any questions regarding any of our open source projects. Eyevinn's consulting business can also offer you:

- Further development of this component
- Customization and integration of this component into your platform
- Support and maintenance agreement

Contact [sales@eyevinn.se](mailto:sales@eyevinn.se) if you are interested.

## About Eyevinn Technology

Eyevinn Technology is an independent consultant firm specialized in video and streaming. Independent in a way that we are not commercially tied to any platform or technology vendor.

At Eyevinn, every software developer consultant has a dedicated budget reserved for open source development and contribution to the open source community. This give us room for innovation, team building and personal competence development. And also gives us as a company a way to contribute back to the open source community.

Want to know more about Eyevinn and how it is to work here. Contact us at work@eyevinn.se!
