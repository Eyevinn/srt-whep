#!/bin/bash
# Bring up srt-whep + a source of the given H.264 profile, retrying until the
# pipeline links cleanly (works around the intermittent tsdemux no-more-pads race,
# known_limitations #6). Leaves server + source running (disowned) and prints the
# offered profile-level-id.
#
# Usage: bringup.sh <profile> [source-script]
#   profile        x264 profile (source-x264.sh) or VT profile (source-file.sh)
#   source-script  default source-x264.sh; use source-file.sh for VideoToolbox
DIR="$(cd "$(dirname "$0")" && pwd)"
. "$DIR/env.sh"
REPO="$(git -C "$DIR" rev-parse --show-toplevel)"
WORK="${WORK:-$REPO/target/codec-test}"; mkdir -p "$WORK"
PROFILE="${1:-constrained-baseline}"
SRC="${2:-$DIR/source-x264.sh}"
SRV_LOG="$WORK/server.log"; SRC_LOG="$WORK/source.log"

cleanup(){
  pkill -9 -f 'target/release/srt-whep' 2>/dev/null
  pkill -9 -f 'gst-launch-1.0.*srtsink'  2>/dev/null
  pkill -9 -f 'ffmpeg.*mpegts'           2>/dev/null
}

for attempt in $(seq 1 6); do
  cleanup; sleep 3
  "$DIR/server.sh" > "$SRV_LOG" 2>&1 & disown
  for i in $(seq 1 10); do sleep 1; lsof -nP -iTCP:8000 2>/dev/null | grep -qi listen && break; done
  "$SRC" "$PROFILE" > "$SRC_LOG" 2>&1 & disown
  sleep 6
  if grep -q "Successfully linked stream" "$SRV_LOG" && ! grep -q "Failed to link" "$SRV_LOG"; then
    # Poll the offer a few times: right after link the branch infra may still be
    # warming up ("Pipeline is not initialized"), and a transient SRT reconnect can
    # briefly EOS-reset the pipeline. Only accept once we get a real offer.
    OFFER=""
    for t in $(seq 1 8); do
      OFFER=$(curl -s -X POST http://localhost:8000/channel -H 'Content-Type: application/sdp' --data '' \
              | grep -oiE 'profile-level-id=[0-9a-f]+' | head -1)
      [ -n "$OFFER" ] && break
      sleep 1
    done
    if [ -n "$OFFER" ]; then
      echo "READY  profile=$PROFILE  attempt=$attempt  offered=$OFFER"
      echo "Player:  ( cd $DIR/../player && python3 -m http.server 8080 --bind 127.0.0.1 )  ->  http://localhost:8080/"
      echo "Logs:    $SRV_LOG , $SRC_LOG"
      exit 0
    fi
    echo "attempt $attempt: linked but offer not served (SRT churn) -> retry"
  fi
  echo "attempt $attempt: tsdemux link race -> retry"
done
echo "FAILED to link $PROFILE cleanly after 6 attempts"; cleanup; exit 1
