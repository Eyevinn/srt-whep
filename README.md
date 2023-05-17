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

To run you need to set the `GST_PLUGIN_PATH` environment variable to where you have the gstreamer plugins installed, e.g:

```
export GST_PLUGIN_PATH=/opt/homebrew/lib/gstreamer-1.0
```

Then run the application. 
```
cargo run | bunyan
```

A http server will be started on port 8000. You can then set up the WebRTC stream.
```
gst-launch-1.0 -v \
    avfvideosrc capture-screen=true ! video/x-raw,framerate=20/1 ! videoscale ! videoconvert ! x264enc tune=zerolatency ! mux. \
    audiotestsrc ! audio/x-raw, format=S16LE, channels=2, rate=44100 ! audioconvert ! voaacenc ! aacparse ! mux. \
    mpegtsmux name=mux ! rtpmp2tpay ! whipsink whip-endpoint="http://localhost:8000/subscriptions"

gst-launch-1.0 -v whepsrc whep-endpoint="http://localhost:8000/subscriptions" \
    video-caps = "application/x-rtp, media=(string)video, clock-rate=(int)90000, encoding-name=(string)H264, payload=(int)96" \
    audio-caps = "application/x-rtp, media=(string)audio, encoding-name=(string)AAC, payload=(int)111" ! \
    rtpmp2tdepay ! decodebin name=d \
    d. ! queue ! autovideosink sync=false \
    d. ! queue ! audioconvert ! autoaudiosink sync=false
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
- [ ] Write the pipeline in Rust
- [ ] Add optional pass-through to another SRT receiver
- [ ] Add support for multiple WebRTC clients

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