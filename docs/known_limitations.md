## Known Issues and Solutions
While using the program, you might encounter some issues. We have documented these problems and their corresponding solutions below:

1. **Ubuntu - Chrome WebRTC Negotiation Issue:**
- Problem: On Ubuntu, playing content using Chrome might lead to a WebRTC negotiation failure, displaying the message "Error resolving 'xxx.local': Name or service not known." This occurs because Chrome mandates the use of anonymized addresses (mDNS hostnames) instead of local IP addresses for WebRTC servers. This measure prevents IP leakage to internet-facing web browsers.
- Solution: To address mDNS-related issues, consider disabling mDNS in Chrome. To do this,  follow these steps:

  1. Open your Chrome browser.
  2. In the address bar, enter chrome://flags and hit Enter.
  3. Search for the setting "mDNS."
  4. Confirm that the setting is marked as "Disabled."
  5. Relaunch Chrome.

2. **MacOS - Safari WebRTC Negotiation Issue:**
- Problem: When utilizing Safari on MacOS, you might encounter a WebRTC negotiation failure with the message "Failed to set remote video description send parameters for m-section with mid='video0'." This issue pertains to video codecs and profiles supported by Safari, which has a limited range of codecs it can handle.
- Solution: For further insights into this issue, refer to this [documentation](supported_codecs.md) that provides detailed information about video codecs and profiles supported by browsers.

3. **Running in Docker on MacOS:**
- Problem: Running the program from a Docker container needs the host-network mode, which is unsupported on Mac systems.
- Solution: For quick testing, we recommend running the program on an Ubuntu system. Mac users can follow our provided build instructions and employ Chrome for playback. Ubuntu users have the flexibility to build from source or use Docker for testing.

4. **Resource Deallocation on Viewer Reload:**
- Problem: Our [WebRTC player](https://webrtc.player.eyevinn.technology/?type=whep) assumes that viewers will stop playing streams by clicking the stop button before leaving. The allocated resources are released via a DELETE request upon stream completion. However, if a viewer accidentally or intentionally reloads the page without stopping the stream, resources might not be deallocated until the SRT client disconnects (The entire pipeline re-runs upon receiving an end-of-stream (EOS) message).
- Solution: Ensure that viewers follow the intended workflow of stopping the stream using the provided controls before reloading or leaving the page.

5. **Chrome WebRTC Connection Retry:**
- Problem: Chrome will automatically retry a broken WebRTC connection, which could lead to complications when the SRT client (caller) disconnects and then reconnects.
- Solution: To mitigate potential issues, it's recommended to reload the page when the SRT input stream is changed.

6. **`tsdemux` split `no-more-pads` on mid-stream join:**
- Problem: When srt-whep connects to an already-running SRT source, `tsdemux` can expose one media pad (e.g. audio) and fire `no-more-pads` before the other pad (video) appears, then fire `no-more-pads` a second time. The pipeline links the first `no-more-pads` and the second one collides with the already-built elements (`Pad was already linked` / `Failed to add elements` in the logs). A link failure posts no error to the bus, so the supervisor does not restart, and the pipeline stays half-linked — `POST /channel` then returns 503. This is intermittent and timing-dependent.
- Solution: Start (or restart) srt-whep so it connects at/near the start of the stream, where the PAT/PMT and both elementary streams appear together. Restarting the source right before srt-whep also resolves it.

7. **GStreamer version / `rswebrtc` plugin:**
- Problem: srt-whep uses the `whipclientsink` from whichever `rswebrtc` plugin the GStreamer installation provides (it no longer compiles its own copy — see [ADR 0003](adr/0003-webrtc-plugin-from-installation.md)). If the installation lacks `rswebrtc`, or a second, mismatched `rswebrtc` is placed earlier on `GST_PLUGIN_PATH`, WHEP can connect but deliver no media.
- Solution: Use a GStreamer build that bundles `rswebrtc` (`gst-inspect-1.0 whipclientsink` should succeed) and keep `GST_PLUGIN_PATH` pointed at that installation only — do not prepend a separately-built `gst-plugins-rs`.
