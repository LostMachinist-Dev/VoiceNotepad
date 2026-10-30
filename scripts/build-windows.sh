#!/usr/bin/env bash
# Cross-compile VoiceNotepad.exe for 64-bit Windows from Linux.
#
# Needs: rustup target x86_64-pc-windows-gnu and g++-mingw-w64-x86-64.
# See HANDOFF.md for the apt list.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

TARGET=x86_64-pc-windows-gnu
PROFILE="${1:-release}"

# ---- cross compiler -----------------------------------------------------
export CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc
export CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32-g++
export AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc

# ---- ONNX Runtime -------------------------------------------------------
# Nothing to link: `ort` is built with `load-dynamic` for Windows, so the exe
# loads onnxruntime.dll at runtime. The DLL is fetched by
# scripts/fetch-onnxruntime.sh and shipped by the installer.

echo "building $TARGET ($PROFILE)"
if [[ "$PROFILE" == "release" ]]; then
  cargo build --target "$TARGET" --release
  OUT="target/$TARGET/release/VoiceNotepad.exe"
else
  cargo build --target "$TARGET"
  OUT="target/$TARGET/debug/VoiceNotepad.exe"
fi

echo
echo "built: $ROOT/$OUT"
ls -lh "$OUT"
