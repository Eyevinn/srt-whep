# SRT to WHEP
This application ingests one MPEG-TS over SRT stream and outputs to WebRTC recvonly clients using WHEP as signaling protocol. Example of use cases:

- Browser based confidence monitor of an incoming stream
- Program or preview output monitor in a browser or tablet

Supports SRT streams in caller and listener mode.
Runs on MacOS and Ubuntu.

![screenshot](docs/screenshot.png)

## Build from Source
### OSX

Requirements:
- XCode command line tools installed
- GStreamer [binaries](https://gstreamer.freedesktop.org/data/pkg/osx/) from GStreamer's website installed
- Rust and cargo installed

Build with Cargo

```
cargo update
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
apt-get -y install pkg-config \
  libssl-dev \
  libunwind-dev \
  libgstreamer1.0-dev \
  libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-base \
  gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-bad \
  gstreamer1.0-plugins-ugly \
  gstreamer1.0-libav \
  libgstrtspserver-1.0-dev \
  libges-1.0-dev
```

Build with Cargo

```
cargo update
cargo install bunyan # Optional, for pretty printing of logs
cargo build --release
```

The binary is then available at `./target/release/srt-whep`. See below for how to run it.

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

This also expects the SRT address `127.0.0.1:8888` to be running in listener mode.

## Debugging

- If you doubt a plugin is missing you can check it using `gst-inspect-1.0 <plugin>`. For example, `gst-inspect-1.0 srtsink`.
- It's possible to generate a test SRT stream using GStreamer for debugging purpose. Please refer to [macOS](docs/SRT_macOS.md) or [Ubuntu](docs/SRT_Ubuntu.md).

- To get more verbose logging you can set the `GST_DEBUG` environment variable to `2`. For example, run in terminal: `export GST_DEBUG=2`

## Issues
All relevant discussions are tracked in [issues](https://github.com/Eyevinn/srt-whep/issues/). Please feel free to open a new issue if you have any questions or problems.

- For Ubuntu users, please notice that `high` video profile is not supported by broswers. Related discussions can be found [here](https://askubuntu.com/questions/1412934/webrtc-h-264-high-profile-doesnt-want-to-play-in-browser).
- For Ubuntu users, if you run into issues with the discoverer, please try to turn it off by setting `enable_discoverer` into `false` in `src/discover.conf` (This should be fixed in the future).
- The application runs only on Chrome now but we will try to support more browsers.
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
