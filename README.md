# SRT to WebRTC
This application ingests one MPEG-TS over SRT stream and outputs to WebRTC recvonly clients using WHEP as signaling protocol.

## Build
### OSX
Requirements:
- XCode command line tools installed
- Install GStreamer using Homebrew
- Install Rust using rustup

```
brew install gstreamer gst-plugins-bad gst-plugins-good gst-plugins-ugly gst-libav
```

## Run

To generate SRT stream, you need to set the `GST_PLUGIN_PATH` environment variable to where you have the gstreamer plugins installed, e.g:

```
export GST_PLUGIN_PATH=/opt/homebrew/lib/gstreamer-1.0

gst-launch-1.0 -v \
    avfvideosrc capture-screen=true ! video/x-raw,framerate=20/1 ! timeoverlay ! videoscale ! videoconvert ! x264enc tune=zerolatency ! video/x-h264, profile=main mux. \
    audiotestsrc ! audio/x-raw, format=S16LE, channels=2, rate=44100 ! audioconvert ! voaacenc ! aacparse ! mux. \
    mpegtsmux name=mux ! queue ! srtserversink uri="srt://127.0.0.1:1234?mode=listener" wait-for-connection=false
```

Then run the application. 
```
GST_DEBUG=1 cargo run -- -i 127.0.0.1:1234 -o :8888 -p 8000 | bunyan
```

The whep server will be started on port 8000. You can then play it out using WHEP [Player](https://webrtc.player.eyevinn.technology/?type=whep). 

The pass-through SRT stream can be viewed using the following command:
```
gst-launch-1.0 -v playbin  uri="srt://127.0.0.1:8888"
```

## Example
![Example](./docs/Example.gif)

## Plans
- [x] Understand WHEP endoint for WebRTC based streaming.
- [x] Check avaible tools for SRT to WebRTC
- [x] Build a prototype server for WHEP
- [x] Build the HTTP server in Rust
- [x] Add support for audio
- [x] Check the format/codec of the SRT stream
- [x] Write the pipeline in Rust
- [x] Add optional pass-through to another SRT receiver
- [x] Test with browser
- [ ] Add support for graceful shutdown
- [ ] Add support for multiple WebRTC clients

## Sample Pipeline
![Pipeline](./docs/pipeline.svg)

## Issues
All relavant discussions are tracked in the [issues](https://github.com/Eyevinn/srt-whep/issues/)

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