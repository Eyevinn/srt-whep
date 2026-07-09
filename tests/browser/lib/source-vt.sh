#!/bin/sh
# SRT test source (caller) via ffmpeg + Apple VideoToolbox (LIVE encode).
# Arg 1 = h264_videotoolbox profile: constrained_baseline | baseline | main | high | constrained_high
# NOTE: a live VT encoder holds VideoToolbox sessions; after a few WHEP offers the
# sink stops producing offers (HTTP 503). For repeated testing use source-file.sh.
PROFILE="${1:-constrained_high}"
echo ">>> ffmpeg/VideoToolbox LIVE SRT source, profile=$PROFILE"
exec ffmpeg -hide_banner -loglevel warning -re \
  -f lavfi -i "testsrc=size=1280x720:rate=30" \
  -f lavfi -i "sine=frequency=1000:sample_rate=44100" \
  -c:v h264_videotoolbox -profile:v "$PROFILE" -allow_sw 1 -realtime 1 -g 30 -b:v 3000k -pix_fmt yuv420p \
  -c:a aac -b:a 128k \
  -f mpegts "srt://127.0.0.1:1234?mode=caller&pkt_size=1316"
