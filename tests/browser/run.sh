#!/usr/bin/env bash
# One-shot automated browser WHEP test:
#   bring up SRT source + srt-whep -> serve the diagnostic player -> drive the
#   real system Chrome through the WHEP handshake -> check the stages -> teardown.
# Exit code is the driver's verdict (0 pass / 1 fail). Teardown always runs.
#
# Usage: run.sh [--profile <p>] [--timeout <sec>] [--headed]
#               [--endpoint <url>] [--skip-bringup] [--player-port <n>]
set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"
REPO="$(git -C "$DIR" rev-parse --show-toplevel)"

PROFILE=constrained-baseline
TIMEOUT=5
PLAYER_PORT=8080
ENDPOINT=""
SKIP_BRINGUP=0
HEADED=""
while [ $# -gt 0 ]; do
  case "$1" in
    --profile)      PROFILE="$2"; shift 2 ;;
    --timeout)      TIMEOUT="$2"; shift 2 ;;
    --player-port)  PLAYER_PORT="$2"; shift 2 ;;
    --endpoint)     ENDPOINT="$2"; shift 2 ;;
    --skip-bringup) SKIP_BRINGUP=1; shift ;;
    --headed)       HEADED="--headed"; shift ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done
[ -n "$ENDPOINT" ] || ENDPOINT="http://localhost:8000/channel"

HTTP_PID=""
teardown() {
  [ -n "$HTTP_PID" ] && kill "$HTTP_PID" 2>/dev/null || true
  pkill -9 -f 'target/release/srt-whep'  2>/dev/null || true
  pkill -9 -f 'gst-launch-1.0.*srtsink'  2>/dev/null || true
  pkill -9 -f 'ffmpeg.*mpegts'           2>/dev/null || true
}
trap teardown EXIT

# 1. dependency
if [ ! -d "$DIR/node_modules" ]; then
  echo ">>> installing puppeteer-core..."
  ( cd "$DIR" && npm install --silent )
fi

# 2. bring up source + srt-whep (retries the tsdemux race)
if [ "$SKIP_BRINGUP" -eq 0 ]; then
  echo ">>> bringing up srt-whep + x264 source (profile=$PROFILE)..."
  "$DIR/lib/bringup.sh" "$PROFILE"
fi

# 3. serve the player
echo ">>> serving player on 127.0.0.1:$PLAYER_PORT..."
( cd "$DIR/player" && exec python3 -m http.server "$PLAYER_PORT" --bind 127.0.0.1 ) >/dev/null 2>&1 &
HTTP_PID=$!
for _ in $(seq 1 10); do
  curl -s -o /dev/null "http://127.0.0.1:$PLAYER_PORT/" && break
  sleep 0.3
done

# 4. drive Chrome and check the stages (its exit code is this script's verdict)
echo ">>> driving Chrome..."
node "$DIR/drive-chrome.mjs" \
  --url "http://localhost:$PLAYER_PORT/" \
  --endpoint "$ENDPOINT" \
  --profile "$PROFILE" \
  --timeout "$TIMEOUT" \
  --json "$REPO/target/codec-test/whep-auto-$PROFILE.json" \
  $HEADED
