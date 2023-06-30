To generate SRT stream on Ubuntu, you can use the following command:
```
gst-launch-1.0 -v videotestsrc ! clockoverlay ! video/x-raw, height=360, width=640 ! videoconvert ! \
    x264enc tune=zerolatency ! video/x-h264, profile=main ! \
    mpegtsmux ! srtsink uri=srt://:1234 wait-for-connection=false
```
