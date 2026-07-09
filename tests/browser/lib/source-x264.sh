#!/bin/sh
# SRT test source (caller) via x264enc. Connects to srt-whep's listener on :1234.
# Arg 1 = x264 profile: constrained-baseline | baseline | main | high | high-4:4:4
DIR="$(cd "$(dirname "$0")" && pwd)"
. "$DIR/env.sh"
PROFILE="${1:-constrained-baseline}"

# high-4:4:4 needs 4:4:4 chroma; every other profile uses 4:2:0.
case "$PROFILE" in
  *4:4:4*) CHROMA="videoconvert ! video/x-raw,format=Y444" ;;
  *)       CHROMA="videoconvert" ;;
esac

echo ">>> x264 SRT source, profile=$PROFILE"
exec gst-launch-1.0 -v \
  videotestsrc is-live=true ! video/x-raw,height=720,width=1280 ! clockoverlay ! $CHROMA ! \
    x264enc tune=zerolatency key-int-max=30 ! video/x-h264,profile=$PROFILE ! mux. \
  audiotestsrc is-live=true ! audio/x-raw,format=S16LE,channels=2,rate=44100 ! audioconvert ! voaacenc ! aacparse ! mux. \
  mpegtsmux name=mux ! queue ! srtsink uri="srt://127.0.0.1:1234?mode=caller" wait-for-connection=false latency=0
