#!/usr/bin/env bash
# Remove what install-linux.sh put in place. Leaves your settings and the
# downloaded speech model alone; the last lines say how to remove those too.
set -euo pipefail

BIN_DIR="$HOME/.local/bin"
APP_DIR="$HOME/.local/share/applications"
ICON_ROOT="$HOME/.local/share/icons/hicolor"

rm -f "$BIN_DIR/voice-notepad"
rm -f "$APP_DIR/voice-notepad.desktop"
for size in 16 24 32 48 64 128 256; do
  rm -f "$ICON_ROOT/${size}x${size}/apps/voice-notepad.png"
done

command -v update-desktop-database >/dev/null && update-desktop-database "$APP_DIR" 2>/dev/null || true
command -v gtk-update-icon-cache  >/dev/null && gtk-update-icon-cache -f -t "$ICON_ROOT" 2>/dev/null || true

echo "removed the program, its menu entry and its icons."
echo
echo "your settings and the downloaded speech model are still here:"
echo "  ~/.config/VoiceNotepad"
echo "  ~/.local/share/VoiceNotepad"
echo "delete those two folders as well if you want it completely gone."
