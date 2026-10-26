#!/usr/bin/env bash
# Install Voice Notepad for the current user on this Linux machine.
#
# Everything goes under ~/.local, so no root is needed and nothing outside
# your home directory is touched. Run scripts/uninstall-linux.sh to undo it.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

BIN_DIR="$HOME/.local/bin"
APP_DIR="$HOME/.local/share/applications"
ICON_ROOT="$HOME/.local/share/icons/hicolor"

echo "building the release binary"
cargo build --release

echo "installing to $BIN_DIR/voice-notepad"
mkdir -p "$BIN_DIR"
install -m 755 target/release/VoiceNotepad "$BIN_DIR/voice-notepad"

echo "installing icons"
for size in 16 24 32 48 64 128 256; do
  dir="$ICON_ROOT/${size}x${size}/apps"
  mkdir -p "$dir"
  install -m 644 "assets/icons/voice-notepad-$size.png" "$dir/voice-notepad.png"
done

echo "installing the desktop entry"
mkdir -p "$APP_DIR"
cat > "$APP_DIR/voice-notepad.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Version=1.0
Name=Voice Notepad
GenericName=Text Editor
Comment=Notepad with local speech to text
Exec=$BIN_DIR/voice-notepad %f
Icon=voice-notepad
Terminal=false
Categories=Utility;TextEditor;
MimeType=text/plain;text/markdown;
Keywords=notepad;dictation;speech;transcribe;voice;
StartupNotify=true
StartupWMClass=voice-notepad
DESKTOP
chmod 644 "$APP_DIR/voice-notepad.desktop"

# Refresh the menu and icon caches, if the tools are around. Harmless if not.
command -v update-desktop-database >/dev/null && update-desktop-database "$APP_DIR" 2>/dev/null || true
command -v gtk-update-icon-cache  >/dev/null && gtk-update-icon-cache -f -t "$ICON_ROOT" 2>/dev/null || true

echo
echo "installed."
echo "  command:  voice-notepad [file]"
echo "  menu:     Voice Notepad"
if ! echo ":$PATH:" | grep -q ":$BIN_DIR:"; then
  echo
  echo "note: $BIN_DIR is not on your PATH, so the command will only work"
  echo "      by its full path until you log out and back in."
fi
