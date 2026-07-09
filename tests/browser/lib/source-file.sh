#!/bin/sh
# SRT test source (caller) that loop-streams a PRE-ENCODED clip with NO re-encode.
# Pre-encodes the clip once (VideoToolbox) if missing, then streams with -c copy so
# no live VT session is held (avoids the offer-timeout exhaustion of source-vt.sh).
# Arg 1 = h264_videotoolbox profile (default constrained_high).
# Caveat: the -stream_loop seam sends EOS -> srt-whep resets its pipeline briefly.
DIR="$(cd "$(dirname "$0")" && pwd)"
. "$DIR/env.sh"
REPO="$(git -C "$DIR" rev-parse --show-toplevel)"
WORK="${WORK:-$REPO/target/codec-test}"; mkdir -p "$WORK"
PROFILE="${1:-constrained_high}"
CLIP="$WORK/clip_$PROFILE.ts"

if [ ! -s "$CLIP" ]; then
  echo ">>> pre-encoding $PROFILE clip (VideoToolbox, one-time) -> $CLIP"
  ffmpeg -hide_banner -loglevel error -y \
    -f lavfi -i "testsrc=size=1280x720:rate=30:duration=20" \
    -f lavfi -i "sine=frequency=1000:sample_rate=44100:duration=20" \
    -c:v h264_videotoolbox -profile:v "$PROFILE" -allow_sw 1 -realtime 0 -g 30 -b:v 3000k -pix_fmt yuv420p \
    -c:a aac -b:a 128k -f mpegts "$CLIP" || { echo "encode failed"; exit 1; }
fi

echo ">>> loop-streaming $CLIP (-c copy, no live encode)"
exec ffmpeg -hide_banner -loglevel warning -re -stream_loop -1 -i "$CLIP" -c copy \
  -f mpegts "srt://127.0.0.1:1234?mode=caller&pkt_size=1316"
