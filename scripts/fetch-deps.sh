#!/usr/bin/env bash
# Downloads the native pieces Cutdeck bundles (macOS / Linux):
#   - FFmpeg + FFprobe  -> src-tauri/binaries/cutdeck-{ffmpeg,ffprobe}-<target-triple>
#   - ONNX Runtime      -> src-tauri/resources/onnxruntime/
#   - whisper.cpp CLI   -> src-tauri/resources/whisper/   (auto captions; models download on first use)
# Usage: scripts/fetch-deps.sh [target-triple]   (defaults to this machine)
#
# FFmpeg must include libvidstab (stabilization), libass (burned-in captions) and,
# on macOS, h264_videotoolbox. The old evermeet.cx /getrelease API returns 410, and
# Martin Riedl's Apple Silicon build has VideoToolbox but not vidstab, so those are
# not used as the primary macOS sources.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TRIPLE="${1:-$(rustc -vV | sed -n 's/host: //p')}"
ORT_VER=1.22.0
WHISPER_TAG=b5454   # same tag as scripts/fetch-deps.ps1
BIN="$ROOT/src-tauri/binaries"
ORT_DIR="$ROOT/src-tauri/resources/onnxruntime"
WH_DIR="$ROOT/src-tauri/resources/whisper"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$BIN" "$ORT_DIR"
get() { echo "  ↓ $1"; curl -fL --retry 5 --retry-delay 2 --retry-all-errors -o "$2" "$1"; }

if ! command -v file >/dev/null 2>&1 || ! command -v python3 >/dev/null 2>&1; then
  echo "fetch-deps.sh needs python3 and file." >&2
  exit 1
fi

echo "Target: $TRIPLE"
case "$TRIPLE" in
  x86_64-unknown-linux-gnu)  FF=linux64;    ORT=linux-x64;     WH_ARCHIVE=whisper-bin-ubuntu-x64.tar.gz; ARCH_TOKEN=x86-64 ;;
  aarch64-unknown-linux-gnu) FF=linuxarm64; ORT=linux-aarch64; WH_ARCHIVE=whisper-bin-ubuntu-arm64.tar.gz; ARCH_TOKEN=aarch64 ;;
  aarch64-apple-darwin)      FF=mac-arm64;  ORT=osx-arm64;     ARCH_TOKEN=arm64 ;;
  x86_64-apple-darwin)       FF=mac-amd64;  ORT=osx-x86_64;    ARCH_TOKEN=x86_64 ;;
  *) echo "Unsupported target $TRIPLE" >&2; exit 1 ;;
esac

has_string() {
  python3 - "$1" "$2" <<'PY'
import sys
needle = sys.argv[2].encode()
with open(sys.argv[1], "rb") as f:
    data = f.read()
sys.exit(0 if needle in data else 1)
PY
}

require_strings() {
  local file="$1" needle
  shift
  for needle in "$@"; do
    if ! has_string "$file" "$needle"; then
      echo "FFmpeg is missing required component '$needle': $file" >&2
      return 1
    fi
  done
}

expect_arch() {
  local desc
  desc="$(file "$1")"
  if ! printf '%s\n' "$desc" | grep -q -F -- "$2"; then
    echo "Wrong architecture for $1 (wanted '$2'):" >&2
    echo "$desc" >&2
    return 1
  fi
}

install_pair() {
  local ff="$1" fp="$2"
  chmod +x "$ff" "$fp"
  if command -v xattr >/dev/null 2>&1; then
    xattr -c "$ff" "$fp" 2>/dev/null || true
  fi
  cp "$ff" "$BIN/cutdeck-ffmpeg-$TRIPLE"
  cp "$fp" "$BIN/cutdeck-ffprobe-$TRIPLE"
  chmod +x "$BIN/cutdeck-ffmpeg-$TRIPLE" "$BIN/cutdeck-ffprobe-$TRIPLE"
}

try_mac_zip() {
  local ff_url="$1" fp_url="$2" arch="$3"
  rm -rf "$TMP/mac"
  mkdir -p "$TMP/mac/ff" "$TMP/mac/fp"
  get "$ff_url" "$TMP/ffmpeg.zip" || return 1
  get "$fp_url" "$TMP/ffprobe.zip" || return 1
  unzip -oq -j "$TMP/ffmpeg.zip" -d "$TMP/mac/ff" || return 1
  unzip -oq -j "$TMP/ffprobe.zip" -d "$TMP/mac/fp" || return 1
  [[ -f $TMP/mac/ff/ffmpeg && -f $TMP/mac/fp/ffprobe ]] || return 1
  expect_arch "$TMP/mac/ff/ffmpeg" "$arch" || return 1
  require_strings "$TMP/mac/ff/ffmpeg" vidstabdetect vidstabtransform h264_videotoolbox libass || return 1
  install_pair "$TMP/mac/ff/ffmpeg" "$TMP/mac/fp/ffprobe"
}

echo "FFmpeg"
if [[ $FF == linux* ]]; then
  get "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-${FF}-gpl.tar.xz" "$TMP/ff.tar.xz"
  mkdir -p "$TMP/ff"
  tar xJf "$TMP/ff.tar.xz" -C "$TMP/ff" --strip-components=1
  install_pair "$TMP/ff/bin/ffmpeg" "$TMP/ff/bin/ffprobe"
  expect_arch "$BIN/cutdeck-ffmpeg-$TRIPLE" "$ARCH_TOKEN"
  require_strings "$BIN/cutdeck-ffmpeg-$TRIPLE" vidstabdetect vidstabtransform libass libx264
  "$BIN/cutdeck-ffmpeg-$TRIPLE" -hide_banner -version | head -1
  if ! "$BIN/cutdeck-ffmpeg-$TRIPLE" -hide_banner -filters 2>/dev/null | grep -q vidstabdetect; then
    echo "Installed FFmpeg cannot list the vidstabdetect filter." >&2
    exit 1
  fi
else
  mac_ok=0
  if [[ $TRIPLE == aarch64-apple-darwin ]]; then
    if try_mac_zip "https://www.osxexperts.net/ffmpeg9arm.zip" "https://www.osxexperts.net/ffprobe9arm.zip" arm64; then
      mac_ok=1
    fi
  else
    html="$(curl -fL --retry 5 --retry-delay 2 --retry-all-errors "https://evermeet.cx/ffmpeg/" || true)"
    ff_name="$(printf '%s\n' "$html" | grep -oE 'ffmpeg-[0-9]+-g[0-9a-f]+\.zip' | head -1 || true)"
    fp_name="$(printf '%s\n' "$html" | grep -oE 'ffprobe-[0-9]+-g[0-9a-f]+\.zip' | head -1 || true)"
    if [[ -n "${ff_name:-}" && -n "${fp_name:-}" ]]; then
      if try_mac_zip "https://evermeet.cx/ffmpeg/$ff_name" "https://evermeet.cx/ffmpeg/$fp_name" x86_64; then
        mac_ok=1
      fi
    fi
    if [[ $mac_ok -ne 1 ]]; then
      if try_mac_zip "https://www.osxexperts.net/ffmpeg80intel.zip" "https://www.osxexperts.net/ffprobe80intel.zip" x86_64; then
        mac_ok=1
      fi
    fi
  fi
  if [[ $mac_ok -ne 1 ]]; then
    echo "Could not download a macOS FFmpeg with vidstab, VideoToolbox and libass for $TRIPLE." >&2
    exit 1
  fi
fi

echo "ONNX Runtime $ORT_VER"
get "https://github.com/microsoft/onnxruntime/releases/download/v${ORT_VER}/onnxruntime-${ORT}-${ORT_VER}.tgz" "$TMP/ort.tgz"
mkdir -p "$TMP/ort"
tar xzf "$TMP/ort.tgz" -C "$TMP/ort" --strip-components=1
rm -f "$ORT_DIR"/libonnxruntime*
if [[ $ORT == osx* ]]; then
  cp "$TMP/ort/lib/libonnxruntime.${ORT_VER}.dylib" "$ORT_DIR/libonnxruntime.dylib"
else
  cp "$TMP/ort/lib/libonnxruntime.so.${ORT_VER}" "$ORT_DIR/libonnxruntime.so"
fi

echo "whisper.cpp $WHISPER_TAG (auto captions)"
rm -rf "$WH_DIR"
mkdir -p "$WH_DIR"
if [[ $FF == linux* ]]; then
  get "https://github.com/ggml-org/whisper.cpp/releases/download/${WHISPER_TAG}/${WH_ARCHIVE}" "$TMP/whisper.tgz"
  mkdir -p "$TMP/whisper"
  tar xzf "$TMP/whisper.tgz" -C "$TMP/whisper"
  cli_src="$(find "$TMP/whisper" -type f -name whisper-cli | head -1)"
  if [[ -z "$cli_src" ]]; then
    echo "whisper-cli was not in the $WH_ARCHIVE archive." >&2
    exit 1
  fi
  src="$(dirname "$cli_src")"
  cp -L "$cli_src" "$WH_DIR/whisper-cli"
  chmod +x "$WH_DIR/whisper-cli"
  for name in libwhisper.so.1 libggml.so.0 libggml-base.so.0; do
    if [[ ! -e "$src/$name" ]]; then
      echo "Missing $name in the whisper.cpp archive." >&2
      exit 1
    fi
    cp -L "$src/$name" "$WH_DIR/$name"
  done
  for cpu in "$src"/libggml-cpu-*.so; do
    [[ -e "$cpu" ]] || continue
    cp -L "$cpu" "$WH_DIR/$(basename "$cpu")"
  done
  if [[ -f "$src/LICENSE" ]]; then
    cp "$src/LICENSE" "$WH_DIR/LICENSE"
  fi
  if command -v patchelf >/dev/null 2>&1; then
    for f in "$WH_DIR"/*; do
      [[ -f "$f" ]] || continue
      if file "$f" | grep -q ELF; then
        patchelf --set-rpath '$ORIGIN' "$f"
      fi
    done
  fi
  if ldd "$WH_DIR/whisper-cli" | grep -q 'not found'; then
    echo "whisper-cli is missing shared libraries:" >&2
    ldd "$WH_DIR/whisper-cli" >&2
    exit 1
  fi
else
  if ! command -v cmake >/dev/null 2>&1; then
    echo "cmake is required to build whisper-cli on macOS (brew install cmake)." >&2
    exit 1
  fi
  if ! command -v git >/dev/null 2>&1; then
    echo "git is required to build whisper-cli on macOS." >&2
    exit 1
  fi
  git clone --depth 1 --branch "$WHISPER_TAG" https://github.com/ggml-org/whisper.cpp "$TMP/whisper.cpp"
  cmake_args=(-DCMAKE_BUILD_TYPE=Release -DGGML_NATIVE=OFF -DBUILD_SHARED_LIBS=OFF -DWHISPER_BUILD_TESTS=OFF -DWHISPER_BUILD_SERVER=OFF)
  if [[ $TRIPLE == x86_64-apple-darwin ]]; then
    # CPU build: Metal cross-compile from an Apple Silicon runner is fragile, and the CLI still works.
    cmake_args+=(-DCMAKE_OSX_ARCHITECTURES=x86_64 -DGGML_METAL=OFF)
    wh_arch=x86_64
  else
    cmake_args+=(-DCMAKE_OSX_ARCHITECTURES=arm64)
    wh_arch=arm64
  fi
  cmake -S "$TMP/whisper.cpp" -B "$TMP/whisper.cpp/build" "${cmake_args[@]}"
  jobs="$(sysctl -n hw.ncpu 2>/dev/null || echo 4)"
  cmake --build "$TMP/whisper.cpp/build" --config Release --target whisper-cli -j "$jobs"
  cli_src="$(find "$TMP/whisper.cpp/build" -type f -name whisper-cli | head -1)"
  if [[ -z "$cli_src" ]]; then
    echo "The whisper.cpp build did not produce whisper-cli." >&2
    exit 1
  fi
  cp "$cli_src" "$WH_DIR/whisper-cli"
  chmod +x "$WH_DIR/whisper-cli"
  if [[ -f "$TMP/whisper.cpp/LICENSE" ]]; then
    cp "$TMP/whisper.cpp/LICENSE" "$WH_DIR/LICENSE"
  fi
  expect_arch "$WH_DIR/whisper-cli" "$wh_arch"
fi

echo "Done."
