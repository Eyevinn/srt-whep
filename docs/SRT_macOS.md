To generate SRT stream on Mac, you need to set the `GST_PLUGIN_PATH`, `GIO_EXTRA_MODULES` and `DYLD_LIBRARY_PATH` environment variable to where you have the gstreamer plugins installed, e.g:

```
export PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/bin:$PATH
export PKG_CONFIG_PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/lib/pkgconfig
export GST_PLUGIN_PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/lib:$GST_PLUGIN_PATH
export GIO_EXTRA_MODULES=/Library/Frameworks/GStreamer.framework/Libraries/gio/modules/
export DYLD_LIBRARY_PATH=$DYLD_LIBRARY_PATH:$GST_PLUGIN_PATH
```

Example command for starting a srt stream with screen captured video and a constant tone as audio.
```
gst-launch-1.0 -v \
    videotestsrc ! clockoverlay ! video/x-raw, height=360, width=640 ! videoconvert ! x264enc tune=zerolatency ! video/x-h264, profile=constrained-baseline ! mux. \
    audiotestsrc ! audio/x-raw, format=S16LE, channels=2, rate=44100 ! audioconvert ! voaacenc ! aacparse ! mux. \
    mpegtsmux name=mux ! queue ! srtsink uri="srt://127.0.0.1:1234?mode=listener" wait-for-connection=false
```
