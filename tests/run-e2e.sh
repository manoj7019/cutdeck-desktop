#!/usr/bin/env bash
# Linux only: runs the UI test against a debug build under a virtual display.
# Needs: tauri-driver (cargo install tauri-driver), webkit2gtk-driver, xvfb, ffmpeg.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$(mktemp -d)"
MEDIA="$WORK/media"; OUT="$WORK/out"; mkdir -p "$MEDIA" "$OUT"
ff() { ffmpeg -hide_banner -loglevel error -y "$@"; }
ff -f lavfi -i testsrc2=s=640x360:r=30:d=120 -f lavfi -i sine=f=440:d=120 -c:v libx264 -preset ultrafast -g 30 -pix_fmt yuv420p -c:a aac -shortest "$MEDIA/long.mp4"
ff -f lavfi -i testsrc=s=360x640:r=25:d=6 -c:v libx264 -preset ultrafast -pix_fmt yuv420p "$MEDIA/portrait.mp4"
ff -f lavfi -i "color=c=0x00d000:s=800x600,drawbox=x=250:y=150:w=300:h=300:color=red:t=fill" -frames:v 1 "$MEDIA/green.png"
ff -f lavfi -i sine=f=220:d=30 "$MEDIA/music.mp3"

(cd "$ROOT/src-tauri" && cargo build)
export CUTDECK_APP="$ROOT/src-tauri/target/debug/cutdeck" CUTDECK_MEDIA="$MEDIA" CUTDECK_TEST_SAVE_DIR="$OUT"
xvfb-run -a -s "-screen 0 1440x900x24" bash -c '
  tauri-driver --port 4444 >/dev/null 2>&1 & D=$!
  sleep 2
  node "'"$ROOT"'/tests/ui-e2e.mjs"; R=$?
  kill $D; exit $R'
echo "Artifacts in $OUT"
