## Useful Commands

1. To generate a testing SRT stream on port 1234 with:
- FFMpeg
```
ffmpeg -f lavfi -re -i testsrc=size=1280x720:rate=30 -f lavfi -re \
       -i sine=frequency=1000:sample_rate=44100 -pix_fmt yuv420p \
       -c:v libx264 -b:v 1000k -g 30 -keyint_min 120 -profile:v baseline -preset veryfast \
       -c:a aac -f mpegts "srt://127.0.0.1:1234?mode=caller&pkt_size=1316"
```
- GStreamer
```
gst-launch-1.0 -v \
    videotestsrc ! clockoverlay ! video/x-raw, height=360, width=640 ! videoconvert ! x264enc tune=zerolatency ! video/x-h264, profile=constrained-baseline ! mux. \
    audiotestsrc ! audio/x-raw, format=S16LE, channels=2, rate=44100 ! audioconvert ! voaacenc ! aacparse ! mux. \
    mpegtsmux name=mux ! queue ! srtsink uri="srt://127.0.0.1:1234?mode=caller" wait-for-connection=false
```
- Our docker image (running in `listener` mode)
```
docker run --rm -p 1234:1234/udp eyevinntechnology/testsrc
```
- **Note that when SRT stream is in caller mode, the player (listener) needs to run first.**

2. To play out an SRT stream on port 1234
- FFPlay
```
ffplay "srt://127.0.0.1:1234?mode=listener"
```
- GStreamer
```
gst-launch-1.0 playbin uri="srt://127.0.0.1:1234?mode=listener"
```
- VLC
1. File -> Open Network
2. Type in URL: ``` srt://127.0.0.1:1234?mode=listener ```

- **To run as caller, change mode from `listener` to `caller`**
ffmpeg -f lavfi -re -i testsrc=size=1280x720:rate=30 -pix_fmt yuv420p \
       -c:v libx264 -b:v 1000k -g 30 -keyint_min 120 -profile:v baseline -preset veryfast \
       -f mpegts "srt://127.0.0.1:1234?mode=caller&pkt_size=1316"
