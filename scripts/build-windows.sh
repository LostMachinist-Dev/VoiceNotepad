#!/usr/bin/env bash
# Cross-compile VoiceNotepad.exe for 64-bit Windows from Linux.
#
# Needs: rustup target x86_64-pc-windows-gnu, g++-mingw-w64-x86-64, cmake,
#        and libclang (for bindgen).  See HANDOFF.md for the apt list.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

TARGET=x86_64-pc-windows-gnu
PROFILE="${1:-release}"

# ---- locate libclang (bindgen loads it at runtime) ------------------------
if [[ -z "${LIBCLANG_PATH:-}" ]]; then
  for d in /usr/lib/x86_64-linux-gnu /usr/lib/llvm-*/lib; do
    if compgen -G "$d/libclang*.so*" > /dev/null; then LIBCLANG_PATH="$d"; break; fi
  done
fi
[[ -n "${LIBCLANG_PATH:-}" ]] || { echo "error: libclang not found (install libclang1-21 or libclang-dev)"; exit 1; }
export LIBCLANG_PATH

# ---- mingw header dirs for bindgen --------------------------------------
# libclang has no Windows system headers of its own, so we hand it mingw's.
MINGW_GCC_INC="$(ls -d /usr/lib/gcc/x86_64-w64-mingw32/*/include 2>/dev/null | head -1)"
MINGW_SYS_INC=/usr/x86_64-w64-mingw32/include
[[ -d "$MINGW_GCC_INC" ]] || { echo "error: mingw gcc headers not found (install g++-mingw-w64-x86-64)"; exit 1; }
[[ -d "$MINGW_SYS_INC" ]] || { echo "error: $MINGW_SYS_INC not found (install mingw-w64-x86-64-dev)"; exit 1; }

export BINDGEN_EXTRA_CLANG_ARGS_x86_64_pc_windows_gnu="--target=x86_64-pc-windows-gnu -isystem $MINGW_GCC_INC -isystem $MINGW_SYS_INC"

# ---- cross compilers for the C/C++ (whisper.cpp) half -------------------
# The -posix variants: whisper.cpp needs std::thread, which libstdc++ only
# implements under the posix threading model. See .cargo/config.toml.
export CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc-posix
export CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32-g++-posix
export AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc-posix

# ---- patched whisper-rs-sys ---------------------------------------------
python3 "$ROOT/scripts/vendor-whisper-sys.py"

echo "building $TARGET ($PROFILE) with libclang at $LIBCLANG_PATH"
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
