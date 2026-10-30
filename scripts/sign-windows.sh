#!/usr/bin/env bash
# Authenticode-sign a Windows exe, from Linux.
#
# Signing is what stops Windows calling the app an unknown publisher. It is
# optional: with no certificate configured this prints why it skipped and
# exits successfully, so the build works either way.
#
# Configure it with environment variables, never a file in the repo:
#
#   export VOICENOTEPAD_CERT=~/secrets/codesign.pfx   # outside the project
#   export VOICENOTEPAD_CERT_PASS='...'               # or leave unset to be asked
#
# usage: sign-windows.sh <file-to-sign>
set -euo pipefail

FILE="${1:?usage: sign-windows.sh <file>}"
[[ -f "$FILE" ]] || { echo "sign: $FILE does not exist"; exit 1; }

if [[ -z "${VOICENOTEPAD_CERT:-}" ]]; then
  echo "sign: skipped (VOICENOTEPAD_CERT is not set) - $(basename "$FILE") is unsigned"
  exit 0
fi
[[ -f "$VOICENOTEPAD_CERT" ]] || { echo "sign: certificate $VOICENOTEPAD_CERT not found"; exit 1; }

# osslsigncode, from the system or from the build tools folder.
TOOLS="${VOICENOTEPAD_TOOLS:-$HOME/.local/share/voicenotepad-build}"
SIGNER="$(command -v osslsigncode || true)"
[[ -n "$SIGNER" ]] || SIGNER="$TOOLS/sign/root/usr/bin/osslsigncode"
[[ -x "$SIGNER" ]] || {
  echo "sign: osslsigncode not found. Install it with:"
  echo "        sudo apt install osslsigncode"
  exit 1
}

# A timestamp is what keeps the signature valid after the certificate expires.
# Without it, everything signed stops verifying on the expiry date.
TIMESTAMP_URL="${VOICENOTEPAD_TIMESTAMP:-http://timestamp.digicert.com}"

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

args=(sign -certs "$VOICENOTEPAD_CERT" -h sha256
      -n "Voice Notepad" -i "https://github.com/" -ts "$TIMESTAMP_URL"
      -in "$FILE" -out "$tmp")
if [[ -n "${VOICENOTEPAD_CERT_PASS:-}" ]]; then
  args+=(-pass "$VOICENOTEPAD_CERT_PASS")
fi

echo "sign: $(basename "$FILE")"
"$SIGNER" "${args[@]}" > /dev/null
mv "$tmp" "$FILE"
trap - EXIT

"$SIGNER" verify "$FILE" 2>&1 | grep -E "Signature verification|Timestamp" || true
