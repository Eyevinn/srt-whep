# Streaming from OBS to Twitch and Previewing Output in Browser

This guide will take you through the step-by-step process of setting up OBS (Open Broadcaster Software) to stream to Twitch and preview the output in broswer using the WHEP (WebRTC HTML5 Player) Web Player.


## Prerequisites

- **Installed OBS:** Download and install OBS from the official website: [OBS Project](https://obsproject.com/).

## Steps

1. **Twitch Account Setup:**

   - If you don't have a Twitch account, create one by visiting: [Twitch Signup](https://www.twitch.tv/signup).
   - Log in to your Twitch account.

2. **Install and Run SRT-WHEP:**

   - You need to run the SRT-WHEP application on your computer. You can either build it from source or use the Docker image (for Ubuntu). Detailed building instructions can be found in the [GitHub repository](https://github.com/Eyevinn/srt-whep#install).
   - To run it in `listener` mode and wait on port 1234, execute the following commands:
     - If building from source: `srt-whep -i 127.0.0.1:1234 -o 127.0.0.1:8888 -p 8000 -s listener | bunyan`
     - If using Docker image: `docker run --rm --network host eyevinntechnology/srt-whep -i 127.0.0.1:1234 -o 127.0.0.1:8888 -p 8000 -s listener`
   - This will forward the SRT stream to port 8888.

3. **Create Input Stream with OBS:**

   - Launch OBS on your computer.
   - In OBS, go to the "Sources" tab and add a new source (e.g., macOS Screen Capture).
   - To connect to SRT-WHEP, navigate to the "Settings" menu in OBS and select the "Stream" tab.
   - Choose "Custom" as your streaming service.
   - In the "Server" field, enter the input SRT URL: `srt://127.0.0.1:1234?mode=caller`
   - Start streaming by clicking the "Start Streaming" button. OBS will begin sending the stream to SRT-WHEP as the caller.
   - You'll see in the SRT-WHEP console that the stream is received and ready for playback.

4. **Stream to Twitch:**

   - Launch another instance of OBS on your computer (e.g., `open -n -a OBS.app` on Mac). If you receive a "OBS is already running" warning, select "Launch anyway."
   - In OBS, go to the "Sources" tab and add a "Media Source."
   - In the "Properties" menu, deselect "Local File" and enter the input URL: `srt://127.0.0.1:8888?mode=listener` (output SRT URL from SRT-WHEP).
   - Click "OK" to add the source, then access the "Settings" menu in OBS.
   - Select the "Stream" tab.
   - Choose "Twitch" as your streaming service and link your Twitch account.
   - Save your settings by clicking "OK" and return to the OBS main window.
   - Initiate streaming by clicking the "Start Streaming" button. OBS will begin sending the stream to Twitch.

5. **Preview the Stream in Browser:**

   - To preview the stream, open the WHEP [Player](https://webrtc.player.eyevinn.technology/?type=whep) in your web browser.
   - Enter the URL: `http://localhost:8000/channel`.
   - Click "Play," and within a few seconds, you should see the OBS stream in your browser.

6. **Viewing the Stream on Twitch:**

   - Feel free to adjust your layout in OBS and make any necessary final changes.
   - Navigate to your Twitch account dashboard to view your live stream.

7. **Ending the Stream:**

   - To stop previewing the stream, click the "Stop" button in the WHEP Web Player.
   - When you're ready to end your stream, return to OBS and click the "Stop Streaming" button.

## Conclusion

Congratulations! You've successfully configured OBS to stream to Twitch and previewed the output using your web browser. Enjoy your seamless streaming experience!
