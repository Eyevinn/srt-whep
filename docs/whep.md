Here we describe how to setup a WebRTC egress session and available tools for testing.

As discussed in this [blog](https://www.meetecho.com/blog/whep-qui/) by [Lorenzo Miniero](https://www.meetecho.com/blog/author/lminiero/), there are two ways to set up a WHEP session.
1. The client originates an SDP offer (stating their intention of receiving specific media), and wait for an answer from the WHEP server, or
2. ask the WHEP server to provide an SDP offer instead (with a description of the media session), and then provide the SDP answer in a further exchange. This approach was axed in the latest WHEP draft.

For testing purpose, we find two WHEP players.
1. [whepsrc](https://gstreamer.freedesktop.org/documentation/webrtchttp/whepsrc.html?gi-language=c) (from gst-plugin-rs ). It takes the initiative and generate an SDP offer.
2. [whep player](https://webrtc.player.eyevinn.technology/) (from Eyevinn). It expects the server to provide an SDP offer.

For end-to-end test, [wrtc-egress](https://github.com/Eyevinn/wrtc-egress) is available.
