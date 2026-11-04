#!/usr/bin/env bash
# Hope Desk variant switcher.
#
# THE APP ICON: replace res/hope-desk/app-icon-1024.png (1024x1024 PNG, transparent corners) --
# that is the only file to swap; this script regenerates AppIcon.icns and flutter/assets/icon.png from it.
#
# Usage: res/hope-desk/apply-variant.sh staff|production
#   staff      -> HopeDesk            org.onehopechurch.hopedesk             "Hope Desk"             hopedesk
#   production -> HopeDeskProduction  org.onehopechurch.hopedesk.production  "Hope Desk Production"  hopedeskproduction
#
# Rewrites the four variant lines in flutter/macos/Runner/Configs/AppInfo.xcconfig.
# Test override: HOPE_DESK_XCCONFIG=/path/to/copy.xcconfig (skips icon generation).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

VARIANT="${1:-}"
case "$VARIANT" in
  staff)
    PRODUCT_NAME="HopeDesk"
    BUNDLE_ID="org.onehopechurch.hopedesk"
    DISPLAY_NAME="Hope Desk"
    URL_SCHEME="hopedesk"
    ;;
  production)
    PRODUCT_NAME="HopeDeskProduction"
    BUNDLE_ID="org.onehopechurch.hopedesk.production"
    DISPLAY_NAME="Hope Desk Production"
    URL_SCHEME="hopedeskproduction"
    ;;
  *)
    echo "usage: $0 staff|production" >&2
    exit 2
    ;;
esac

XCCONFIG="${HOPE_DESK_XCCONFIG:-$ROOT/flutter/macos/Runner/Configs/AppInfo.xcconfig}"
[ -f "$XCCONFIG" ] || { echo "error: $XCCONFIG not found" >&2; exit 1; }

set_key() {
  local key="$1" value="$2"
  python3 - "$XCCONFIG" "$key" "$value" <<'PY'
import sys, re
path, key, value = sys.argv[1:4]
lines = open(path, encoding="utf-8").read().split("\n")
pat = re.compile(r"^" + re.escape(key) + r" = .*$")
hit = False
for i, l in enumerate(lines):
    if pat.match(l):
        lines[i] = f"{key} = {value}"
        hit = True
if not hit:
    sys.exit(f"error: no '{key} = ' line in {path}")
open(path, "w", encoding="utf-8").write("\n".join(lines))
PY
  grep -qxF "$key = $value" "$XCCONFIG" || { echo "error: $key not set to '$value' in $XCCONFIG" >&2; exit 1; }
}

set_key PRODUCT_NAME "$PRODUCT_NAME"
set_key PRODUCT_BUNDLE_IDENTIFIER "$BUNDLE_ID"
set_key HOPE_DESK_DISPLAY_NAME "$DISPLAY_NAME"
set_key HOPE_DESK_URL_SCHEME "$URL_SCHEME"

ICON_NOTE="icon unchanged"
PNG="$ROOT/res/hope-desk/app-icon-1024.png"
if [ -z "${HOPE_DESK_XCCONFIG:-}" ] && [ -f "$PNG" ] && command -v iconutil >/dev/null 2>&1 && command -v sips >/dev/null 2>&1; then
  TMP="$(mktemp -d)"
  trap 'rm -rf "$TMP"' EXIT
  SET="$TMP/AppIcon.iconset"
  mkdir -p "$SET"
  for s in 16 32 64 128 256 512 1024; do
    sips -z "$s" "$s" "$PNG" --out "$TMP/$s.png" >/dev/null
  done
  cp "$TMP/16.png"   "$SET/icon_16x16.png"
  cp "$TMP/32.png"   "$SET/icon_16x16@2x.png"
  cp "$TMP/32.png"   "$SET/icon_32x32.png"
  cp "$TMP/64.png"   "$SET/icon_32x32@2x.png"
  cp "$TMP/128.png"  "$SET/icon_128x128.png"
  cp "$TMP/256.png"  "$SET/icon_128x128@2x.png"
  cp "$TMP/256.png"  "$SET/icon_256x256.png"
  cp "$TMP/512.png"  "$SET/icon_256x256@2x.png"
  cp "$TMP/512.png"  "$SET/icon_512x512.png"
  cp "$TMP/1024.png" "$SET/icon_512x512@2x.png"
  iconutil -c icns "$SET" -o "$ROOT/flutter/macos/Runner/AppIcon.icns"
  sips -z 256 256 "$PNG" --out "$ROOT/flutter/assets/icon.png" >/dev/null
  ICON_NOTE="icon regenerated"
else
  echo "note: icon source or iconutil/sips unavailable (or test override); keeping committed icon files" >&2
fi

echo "Hope Desk variant '$VARIANT': $PRODUCT_NAME / $BUNDLE_ID / $DISPLAY_NAME / $URL_SCHEME ($ICON_NOTE)"
