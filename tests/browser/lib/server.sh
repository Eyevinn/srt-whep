#!/bin/sh
# Run srt-whep as SRT listener on :1234, WHEP on :8000, SRT passthrough on :8888.
DIR="$(cd "$(dirname "$0")" && pwd)"
. "$DIR/env.sh"
REPO="$(git -C "$DIR" rev-parse --show-toplevel)"
export RUST_LOG="${RUST_LOG:-info}"
exec "$REPO/target/release/srt-whep" -i 127.0.0.1:1234 -o 127.0.0.1:8888 -p 8000 -s listener
