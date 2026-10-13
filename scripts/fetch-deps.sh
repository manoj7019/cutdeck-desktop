#!/usr/bin/env bash
# Downloads the native pieces Cutdeck bundles (macOS / Linux):
#   - FFmpeg + FFprobe  -> src-tauri/binaries/cutdeck-{ffmpeg,ffprobe}-<target-triple>
#   - ONNX Runtime 1.22 -> src-tauri/resources/onnxruntime/
# Usage: scripts/fetch-deps.sh [target-triple]   (defaults to this machine)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TRIPLE="${1:-$(rustc -vV | sed -n 's/host: //p')}"
ORT_VER=1.22.0
BIN="$ROOT/src-tauri/binaries"
ORT_DIR="$ROOT/src-tauri/resources/onnxruntime"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$BIN" "$ORT_DIR"
get() { echo "  ↓ $1"; curl -fsSL --retry 3 -o "$2" "$1"; }

echo "Target: $TRIPLE"
case "$TRIPLE" in
  x86_64-unknown-linux-gnu)  FF=linux64;    ORT=linux-x64 ;;
  aarch64-unknown-linux-gnu) FF=linuxarm64; ORT=linux-aarch64 ;;
  aarch64-apple-darwin)      FF=mac-arm64;  ORT=osx-arm64 ;;
  x86_64-apple-darwin)       FF=mac-amd64;  ORT=osx-x86_64 ;;
  *) echo "Unsupported target $TRIPLE"; exit 1 ;;
esac

echo "FFmpeg"
if [[ $FF == linux* ]]; then
  get "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-${FF}-gpl.tar.xz" "$TMP/ff.tar.xz"
  mkdir -p "$TMP/ff" && tar xJf "$TMP/ff.tar.xz" -C "$TMP/ff" --strip-components=1
  cp "$TMP/ff/bin/ffmpeg" "$BIN/cutdeck-ffmpeg-$TRIPLE"
  cp "$TMP/ff/bin/ffprobe" "$BIN/cutdeck-ffprobe-$TRIPLE"
else
  arch=${FF#mac-}
  # Static macOS builds (with VideoToolbox) by Martin Riedl; evermeet.cx (Intel) as fallback.
  if get "https://ffmpeg.martin-riedl.de/redirect/latest/macos/${arch}/release/ffmpeg.zip" "$TMP/ffmpeg.zip" &&
     get "https://ffmpeg.martin-riedl.de/redirect/latest/macos/${arch}/release/ffprobe.zip" "$TMP/ffprobe.zip"; then :
  else
    get "https://evermeet.cx/ffmpeg/getrelease/zip" "$TMP/ffmpeg.zip"
    get "https://evermeet.cx/ffmpeg/getrelease/ffprobe/zip" "$TMP/ffprobe.zip"
  fi
  unzip -oq "$TMP/ffmpeg.zip" -d "$TMP/ff"; unzip -oq "$TMP/ffprobe.zip" -d "$TMP/ff"
  cp "$TMP/ff/ffmpeg" "$BIN/cutdeck-ffmpeg-$TRIPLE"
  cp "$TMP/ff/ffprobe" "$BIN/cutdeck-ffprobe-$TRIPLE"
  xattr -c "$BIN/cutdeck-ffmpeg-$TRIPLE" "$BIN/cutdeck-ffprobe-$TRIPLE" 2>/dev/null || true
fi
chmod +x "$BIN/cutdeck-ffmpeg-$TRIPLE" "$BIN/cutdeck-ffprobe-$TRIPLE"

echo "ONNX Runtime $ORT_VER"
get "https://github.com/microsoft/onnxruntime/releases/download/v${ORT_VER}/onnxruntime-${ORT}-${ORT_VER}.tgz" "$TMP/ort.tgz"
mkdir -p "$TMP/ort" && tar xzf "$TMP/ort.tgz" -C "$TMP/ort" --strip-components=1
rm -f "$ORT_DIR"/libonnxruntime*
if [[ $ORT == osx* ]]; then
  cp "$TMP/ort/lib/libonnxruntime.${ORT_VER}.dylib" "$ORT_DIR/libonnxruntime.dylib"
else
  cp "$TMP/ort/lib/libonnxruntime.so.${ORT_VER}" "$ORT_DIR/libonnxruntime.so"
fi

"$BIN/cutdeck-ffmpeg-$TRIPLE" -hide_banner -version | head -1 || true
echo "Done."
