#!/usr/bin/env bash
# Build VoiceNotepad.exe and wrap it in a Windows installer.
#
# NSIS is not packaged for this box and there is no root access, so the
# official Windows build of makensis is fetched once and run under Wine. The
# ONNX Runtime DLL the app needs at runtime is fetched the same way. That
# is also exactly how it would run on a Windows machine, so the installer is
# byte-for-byte what NSIS would produce there.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
TOOLS="${VOICENOTEPAD_TOOLS:-$HOME/.local/share/voicenotepad-build}"
NSIS_VERSION=3.10
NSIS_DIR="$TOOLS/nsis-$NSIS_VERSION"
MAKENSIS="$NSIS_DIR/makensis.exe"
DIST="$ROOT/dist"

command -v wine >/dev/null || { echo "error: wine is needed to run makensis"; exit 1; }

# ---- fetch NSIS once -----------------------------------------------------
if [[ ! -f "$MAKENSIS" ]]; then
  echo "fetching NSIS $NSIS_VERSION into $TOOLS"
  mkdir -p "$TOOLS"
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  curl -fsSL -o "$tmp/nsis.zip" \
    "https://downloads.sourceforge.net/project/nsis/NSIS%203/$NSIS_VERSION/nsis-$NSIS_VERSION.zip"
  unzip -q "$tmp/nsis.zip" -d "$tmp/x"
  mv "$tmp/x/nsis-$NSIS_VERSION" "$NSIS_DIR"
fi
[[ -f "$MAKENSIS" ]] || { echo "error: $MAKENSIS missing after fetch"; exit 1; }

# ---- ONNX Runtime DLL ----------------------------------------------------
ONNX_DLL="$("$ROOT/scripts/fetch-onnxruntime.sh")"
[[ -f "$ONNX_DLL" ]] || { echo "error: onnxruntime.dll was not fetched"; exit 1; }

# ---- build the exe -------------------------------------------------------
"$ROOT/scripts/build-windows.sh" release
EXE="$ROOT/target/x86_64-pc-windows-gnu/release/VoiceNotepad.exe"
[[ -f "$EXE" ]] || { echo "error: $EXE was not built"; exit 1; }

# Sign the program before it goes into the installer, so the copy that lands
# on disk is signed too. A no-op when no certificate is configured.
"$ROOT/scripts/sign-windows.sh" "$EXE"

# ---- compile the installer ----------------------------------------------
mkdir -p "$DIST"
OUT="$DIST/VoiceNotepad-$VERSION-Setup.exe"

echo "compiling the installer with makensis $NSIS_VERSION under wine"
# NSIS resolves File paths the Windows way, so hand it real Windows paths
# (Z:\...\ with backslashes) rather than the Unix ones.
export WINEDEBUG="${WINEDEBUG:--all}"
win() { winepath -w "$1"; }
touch "$OUT"   # so winepath can resolve it

wine "$MAKENSIS" \
  "-DVERSION=$VERSION" \
  "-DSOURCE_EXE=$(win "$EXE")" \
  "-DONNX_DLL=$(win "$ONNX_DLL")" \
  "-DASSETS=$(win "$ROOT/assets")" \
  "-DLICENSE_FILE=$(win "$ROOT/LICENSE")" \
  "-DOUTFILE=$(win "$OUT")" \
  "$(win "$ROOT/packaging/installer.nsi")"

# And the installer itself, which is the file people actually double-click.
"$ROOT/scripts/sign-windows.sh" "$OUT"

echo
echo "installer: $OUT"
ls -lh "$OUT"
