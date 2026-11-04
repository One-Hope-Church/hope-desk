#!/usr/bin/env bash
# Local test for lipo-merge.sh. Builds two tiny fake .app bundles (arm64 and x86_64) with clang,
# merges them, and asserts the result. Also checks that mismatched inputs fail.
# Usage: res/hope-desk/test-lipo-merge.sh   (needs clang, lipo, codesign: runs on a Mac)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MERGE="$SCRIPT_DIR/lipo-merge.sh"
W="$(mktemp -d)"
trap 'rm -rf "$W"' EXIT
PB=/usr/libexec/PlistBuddy

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "ok: $*"; }

printf 'int main(void){return 0;}\n' > "$W/main.c"
printf 'int hd_value(void){return 7;}\n' > "$W/lib.c"

make_app() { # <dir> <arch>
  local app="$1" arch="$2"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Frameworks/Thing.framework/Versions/A" "$app/Contents/Resources"
  clang -arch "$arch" -o "$app/Contents/MacOS/HopeTest" "$W/main.c"
  clang -arch "$arch" -o "$app/Contents/MacOS/service" "$W/main.c"
  clang -arch "$arch" -dynamiclib -o "$app/Contents/Frameworks/liblibtest.dylib" "$W/lib.c"
  clang -arch "$arch" -dynamiclib -o "$app/Contents/Frameworks/Thing.framework/Versions/A/Thing" "$W/lib.c"
  mkdir -p "$app/Contents/Frameworks/Thing.framework/Versions/A/Resources"
  cat > "$app/Contents/Frameworks/Thing.framework/Versions/A/Resources/Info.plist" <<PL
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>org.onehopechurch.hopedesk.test.thing</string>
<key>CFBundleExecutable</key><string>Thing</string>
<key>CFBundlePackageType</key><string>FMWK</string>
</dict></plist>
PL
  ln -s A "$app/Contents/Frameworks/Thing.framework/Versions/Current"
  ln -s Versions/Current/Resources "$app/Contents/Frameworks/Thing.framework/Resources"
  ln -s Versions/Current/Thing "$app/Contents/Frameworks/Thing.framework/Thing"
  # shared file that is already fat on both sides
  clang -arch arm64 -arch x86_64 -dynamiclib -o "$app/Contents/Frameworks/FatShared.dylib" "$W/lib.c"
  echo "same on both sides" > "$app/Contents/Resources/data.txt"
  cat > "$app/Contents/Info.plist" <<PL
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>org.onehopechurch.hopedesk.test</string>
<key>CFBundleExecutable</key><string>HopeTest</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSMinimumSystemVersion</key><string>$([ "$arch" = arm64 ] && echo 12.3 || echo 10.14)</string>
</dict></plist>
PL
}

make_app "$W/arm/HopeTest.app" arm64
make_app "$W/x86/HopeTest.app" x86_64

# --- happy path
HOPE_DESK_BUNDLE_VERSION="hd2.99" "$MERGE" "$W/arm/HopeTest.app" "$W/x86/HopeTest.app" "$W/out/Hope Test.app" > "$W/merge.log" 2>&1 \
  || { cat "$W/merge.log"; fail "merge failed on valid inputs"; }
cat "$W/merge.log"
OUT="$W/out/Hope Test.app"

sorted() { lipo -archs "$1" | tr ' ' '\n' | sort | tr '\n' ' ' | sed 's/ $//'; }
for f in Contents/MacOS/HopeTest Contents/MacOS/service Contents/Frameworks/liblibtest.dylib \
         Contents/Frameworks/Thing.framework/Versions/A/Thing Contents/Frameworks/FatShared.dylib; do
  [ "$(sorted "$OUT/$f")" = "arm64 x86_64" ] || fail "$f is [$(sorted "$OUT/$f")]"
done
pass "all Mach-O files report arm64 + x86_64"

[ -L "$OUT/Contents/Frameworks/Thing.framework/Versions/Current" ] || fail "framework symlink lost"
[ "$(cat "$OUT/Contents/Resources/data.txt")" = "same on both sides" ] || fail "resource changed"
[ -x "$OUT/Contents/MacOS/HopeTest" ] || fail "executable bit lost"
pass "symlinks, resources and modes preserved"

[ "$($PB -c 'Print :LSMinimumSystemVersion' "$OUT/Contents/Info.plist")" = "10.14" ] || fail "LSMinimumSystemVersion"
[ "$($PB -c 'Print :LSMinimumSystemVersionByArchitecture:arm64' "$OUT/Contents/Info.plist")" = "12.3" ] || fail "min arm64"
[ "$($PB -c 'Print :LSMinimumSystemVersionByArchitecture:x86_64' "$OUT/Contents/Info.plist")" = "10.14" ] || fail "min x86_64"
[ "$($PB -c 'Print :CFBundleVersion' "$OUT/Contents/Info.plist")" = "hd2.99" ] || fail "CFBundleVersion"
pass "Info.plist keys fixed"

codesign --verify --deep --strict "$OUT" || fail "ad-hoc signature does not verify"
pass "ad-hoc signature verifies"

# --- negative: file only on one side
cp -R "$W/x86" "$W/x86-extra"
echo extra > "$W/x86-extra/HopeTest.app/Contents/Resources/only-x86.txt"
if "$MERGE" "$W/arm/HopeTest.app" "$W/x86-extra/HopeTest.app" "$W/out-neg1.app" > "$W/neg1.log" 2>&1; then
  fail "merge should fail when a file exists on one side only"
fi
grep -q "only-x86.txt" "$W/neg1.log" || fail "failure output should name the mismatched file"
pass "file-set mismatch fails and names the file"

# --- negative: both sides arm64
if "$MERGE" "$W/arm/HopeTest.app" "$W/arm/HopeTest.app" "$W/out-neg2.app" > "$W/neg2.log" 2>&1; then
  fail "merge should fail when both bundles are arm64"
fi
pass "same-arch inputs fail"

echo "ALL TESTS PASSED"
