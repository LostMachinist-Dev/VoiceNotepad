#!/usr/bin/env bash
# Build every release artifact and its checksum, into dist/.
#
#   VoiceNotepad-<version>-Setup.exe        Windows installer
#   VoiceNotepad-<version>-portable.zip     Windows, unzipped and run in place
#   voice-notepad-<version>-linux-x86_64.tar.gz   Linux binary and installer
#   SHA256SUMS.txt                          checksums for all three
#
# Run it on Linux with the mingw-w64 toolchain installed; see README.md.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
DIST="$ROOT/dist"
mkdir -p "$DIST"

echo "packaging Voice Notepad $VERSION"

# --- Windows: installer (also builds the exe) ------------------------------
"$ROOT/scripts/build-installer.sh"

# --- Windows: portable zip -------------------------------------------------
# Same exe, plus the ONNX Runtime DLL it loads at startup. The installer packs
# that DLL inside itself; here it has to sit beside the exe.
ONNX_DLL="$("$ROOT/scripts/fetch-onnxruntime.sh")"
STAGE="$DIST/portable-stage"
rm -rf "$STAGE"
mkdir -p "$STAGE"
cp "$ROOT/target/x86_64-pc-windows-gnu/release/VoiceNotepad.exe" "$STAGE/"
cp "$ONNX_DLL" "$STAGE/"
cp "$ROOT/README.md" "$ROOT/LICENSE" "$STAGE/"
[[ -f "$(dirname "$ONNX_DLL")/ONNXRUNTIME-LICENSE.txt" ]] &&
  cp "$(dirname "$ONNX_DLL")/ONNXRUNTIME-LICENSE.txt" "$STAGE/"
cat > "$STAGE/READ ME FIRST.txt" <<'TXT'
Voice Notepad - portable

Run VoiceNotepad.exe. Keep onnxruntime.dll in the same folder; the program
will not start without it.

Windows will warn that the publisher is unknown, because this is not
code-signed. Click "More info" and then "Run anyway".

On first run it downloads the speech model (2.4 GB by default, or a much
smaller one you can pick in Settings). After that it works offline.
TXT
( cd "$STAGE" && zip -qr "$DIST/VoiceNotepad-$VERSION-portable.zip" . )
rm -rf "$STAGE"

# --- Linux: binary tarball -------------------------------------------------
# ONNX Runtime is linked statically on Linux, so the binary stands alone
# apart from the system C library and ALSA.
cargo build --release
LSTAGE="$DIST/linux-stage/voice-notepad-$VERSION"
rm -rf "$DIST/linux-stage"
mkdir -p "$LSTAGE"
cp "$ROOT/target/release/VoiceNotepad" "$LSTAGE/voice-notepad"
cp "$ROOT/README.md" "$ROOT/LICENSE" "$LSTAGE/"
cp -r "$ROOT/assets/icons" "$LSTAGE/"
cat > "$LSTAGE/install.sh" <<'TXT'
#!/usr/bin/env bash
# Install for the current user, under ~/.local. No root needed.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
mkdir -p ~/.local/bin ~/.local/share/applications
install -m755 "$HERE/voice-notepad" ~/.local/bin/voice-notepad
for png in "$HERE"/icons/voice-notepad-*.png; do
  size="${png##*-}"; size="${size%.png}"
  dir=~/.local/share/icons/hicolor/${size}x${size}/apps
  mkdir -p "$dir" && install -m644 "$png" "$dir/voice-notepad.png"
done
cat > ~/.local/share/applications/voice-notepad.desktop <<DESK
[Desktop Entry]
Type=Application
Name=Voice Notepad
Comment=Notepad with local speech to text
Exec=voice-notepad %f
Icon=voice-notepad
Terminal=false
Categories=Utility;TextEditor;
MimeType=text/plain;text/markdown;
DESK
command -v update-desktop-database >/dev/null && update-desktop-database ~/.local/share/applications || true
echo "installed. Run: voice-notepad"
echo "If ~/.local/bin is not on your PATH, add it to ~/.bashrc:"
echo '  export PATH="$HOME/.local/bin:$PATH"'
TXT
chmod +x "$LSTAGE/install.sh"
( cd "$DIST/linux-stage" && tar czf "$DIST/voice-notepad-$VERSION-linux-x86_64.tar.gz" "voice-notepad-$VERSION" )
rm -rf "$DIST/linux-stage"

# --- Checksums -------------------------------------------------------------
( cd "$DIST" && sha256sum \
    "VoiceNotepad-$VERSION-Setup.exe" \
    "VoiceNotepad-$VERSION-portable.zip" \
    "voice-notepad-$VERSION-linux-x86_64.tar.gz" > SHA256SUMS.txt )

echo
echo "artifacts in $DIST:"
ls -lh "$DIST"
echo
cat "$DIST/SHA256SUMS.txt"
