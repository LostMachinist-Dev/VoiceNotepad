#!/usr/bin/env bash
# Fetch the Windows build of ONNX Runtime, which the installer ships beside
# the exe.
#
# Microsoft publishes no MinGW build, so there is nothing to link against when
# cross-compiling. `ort` is built with `load-dynamic` instead and loads this
# DLL at runtime. It is MIT licensed, so redistributing it is fine.
set -euo pipefail

# Must be at least the ONNX Runtime API version `ort` asks for (api-28).
ORT_VERSION="${ORT_VERSION:-1.31.0}"
TOOLS="${VOICENOTEPAD_TOOLS:-$HOME/.local/share/voicenotepad-build}"
DEST="$TOOLS/onnxruntime-$ORT_VERSION"
DLL="$DEST/onnxruntime.dll"

if [[ -f "$DLL" ]]; then
  echo "$DLL"
  exit 0
fi

echo "fetching ONNX Runtime $ORT_VERSION for Windows" >&2
mkdir -p "$DEST"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL -o "$tmp/ort.zip" \
  "https://github.com/microsoft/onnxruntime/releases/download/v$ORT_VERSION/onnxruntime-win-x64-$ORT_VERSION.zip"
unzip -q "$tmp/ort.zip" -d "$tmp/x"
cp "$tmp/x/onnxruntime-win-x64-$ORT_VERSION/lib/onnxruntime.dll" "$DLL"
cp "$tmp/x/onnxruntime-win-x64-$ORT_VERSION/LICENSE" "$DEST/ONNXRUNTIME-LICENSE.txt" 2>/dev/null || true
echo "$DLL"
