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
