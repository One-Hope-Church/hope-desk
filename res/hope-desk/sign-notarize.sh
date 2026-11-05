#!/usr/bin/env bash
# Hope Desk: Developer ID sign, notarize, staple and package one .app into a DMG.
# Runs on Corey's Mac (the signing identity and notary profile live in its keychain).
#
# Usage: res/hope-desk/sign-notarize.sh <path/to/App.app> <out.dmg> "<Volume Name>"
#
# Env: SIGN_ID (default: Developer ID Application: One Hope, Inc (UBH6CJDNMJ))
#      NOTARY_PROFILE (default: onyx-bridge-notary, a notarytool keychain profile)
set -euo pipefail

[ "$#" -eq 3 ] || { echo "usage: $0 <App.app> <out.dmg> <volume name>" >&2; exit 2; }
APP="${1%/}"; DMG="$2"; VOL="$3"
SIGN_ID="${SIGN_ID:-Developer ID Application: One Hope, Inc (UBH6CJDNMJ)}"
NOTARY_PROFILE="${NOTARY_PROFILE:-onyx-bridge-notary}"
[ -d "$APP" ] || { echo "error: $APP not found" >&2; exit 1; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# Same entitlements v1 shipped with.
cat > "$TMP/app.entitlements" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>com.apple.security.app-sandbox</key><false/>
  <key>com.apple.security.cs.allow-jit</key><true/>
  <key>com.apple.security.device.audio-input</key><true/>
  <key>com.apple.security.network.client</key><true/>
</dict></plist>
EOF

sign() { codesign --force --options runtime --timestamp --sign "$SIGN_ID" "$@"; }

# 1. Nested code, inside out: loose dylibs, then framework bundles, then helper executables.
find "$APP/Contents/Frameworks" -maxdepth 1 -type f -name '*.dylib' -print0 |
  while IFS= read -r -d '' f; do sign "$f"; done
find "$APP/Contents/Frameworks" -maxdepth 1 -type d -name '*.framework' -print0 |
  while IFS= read -r -d '' f; do sign "$f"; done
MAIN="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$APP/Contents/Info.plist")"
find "$APP/Contents/MacOS" -type f ! -name "$MAIN" -print0 |
  while IFS= read -r -d '' f; do sign "$f"; done

# 2. The app itself, with entitlements.
sign --entitlements "$TMP/app.entitlements" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

# 3. Notarize + staple the app.
ditto -c -k --keepParent "$APP" "$TMP/app.zip"
xcrun notarytool submit "$TMP/app.zip" --keychain-profile "$NOTARY_PROFILE" --wait
xcrun stapler staple "$APP"

# 4. DMG with an Applications shortcut; sign, notarize, staple.
mkdir -p "$TMP/dmg"
ditto "$APP" "$TMP/dmg/$(basename "$APP")"
ln -s /Applications "$TMP/dmg/Applications"
mkdir -p "$(dirname "$DMG")"
hdiutil create -volname "$VOL" -srcfolder "$TMP/dmg" -fs HFS+ -format UDZO -ov "$DMG" >/dev/null
codesign --force --timestamp --sign "$SIGN_ID" "$DMG"
xcrun notarytool submit "$DMG" --keychain-profile "$NOTARY_PROFILE" --wait
xcrun stapler staple "$DMG"
spctl --assess --type open --context context:primary-signature --verbose "$DMG"
echo "ready: $DMG"
