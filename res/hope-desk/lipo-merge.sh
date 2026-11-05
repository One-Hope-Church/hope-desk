#!/usr/bin/env bash
# Hope Desk universal merge.
#
# Usage: res/hope-desk/lipo-merge.sh <arm64.app> <x86_64.app> <out.app>
#
# Merges two single-architecture builds of the same macOS .app into one universal bundle:
#   1. copies the arm64 bundle to <out.app>;
#   2. requires both bundles to contain exactly the same files (ignoring _CodeSignature/);
#   3. lipo-merges every Mach-O file that is thin-on-both-sides with different archs; keeps the
#      arm64-side copy of files that are already fat on both sides; fails on anything else;
#   4. fixes the Info.plist minimum-version keys (and CFBundleVersion if HOPE_DESK_BUNDLE_VERSION is set);
#   5. verifies every Mach-O in the result reports exactly arm64 + x86_64;
#   6. ad-hoc signs the result (Developer ID signing replaces this later).
#
# Optional env: HOPE_DESK_BUNDLE_VERSION (e.g. hd2.37), HOPE_DESK_MIN_ARM64 (12.3), HOPE_DESK_MIN_X86_64 (10.14).
set -euo pipefail

[ "$#" -eq 3 ] || { echo "usage: $0 <arm64.app> <x86_64.app> <out.app>" >&2; exit 2; }
ARM="${1%/}"; X86="${2%/}"; OUT="${3%/}"
[ -d "$ARM" ] || { echo "error: $ARM is not a directory" >&2; exit 1; }
[ -d "$X86" ] || { echo "error: $X86 is not a directory" >&2; exit 1; }
MIN_ARM64="${HOPE_DESK_MIN_ARM64:-12.3}"
MIN_X86_64="${HOPE_DESK_MIN_X86_64:-10.14}"
PB=/usr/libexec/PlistBuddy

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# Sorted "arch arch" string for a Mach-O file; fails (non-zero) if the file is not Mach-O.
archs_of() {
  local a
  a="$(lipo -archs "$1" 2>/dev/null)" || return 1
  [ -n "$a" ] || return 1
  # shellcheck disable=SC2086
  printf '%s\n' $a | sort | tr '\n' ' ' | sed 's/ $//'
}

file_list() {
  (cd "$1" && find . \( -type f -o -type l \) | grep -v '/_CodeSignature/' | LC_ALL=C sort)
}

# 1. copy arm64 bundle
rm -rf "$OUT"
mkdir -p "$(dirname "$OUT")"
ditto "$ARM" "$OUT"

# 2. same file set on both sides, with one exception: the x86_64 build targets macOS 10.14,
#    so Xcode embeds the Swift runtime (Contents/Frameworks/libswift*.dylib). The arm64 build
#    (12.3+) uses the OS copy. Those x86_64-only dylibs are copied in as-is; only the x86_64
#    slice of the executables ever loads them.
file_list "$ARM" > "$TMP/arm.list"
file_list "$X86" > "$TMP/x86.list"
LC_ALL=C comm -13 "$TMP/arm.list" "$TMP/x86.list" > "$TMP/x86only.list"
LC_ALL=C comm -23 "$TMP/arm.list" "$TMP/x86.list" > "$TMP/armonly.list"
grep -v -E '^\./Contents/Frameworks/libswift[^/]*\.dylib$' "$TMP/x86only.list" > "$TMP/x86only.bad" || true
if [ -s "$TMP/armonly.list" ] || [ -s "$TMP/x86only.bad" ]; then
  echo "error: the arm64 and x86_64 bundles do not contain the same files." >&2
  echo "       '<' = only in arm64 build, '>' = only in x86_64 build:" >&2
  sed 's/^/< /' "$TMP/armonly.list" >&2
  sed 's/^/> /' "$TMP/x86only.bad" >&2
  exit 1
fi
swift_only=0
while IFS= read -r rel; do
  [ "$(archs_of "$X86/$rel")" = "x86_64" ] || { echo "error: $rel is not a thin x86_64 dylib" >&2; exit 1; }
  ditto "$X86/$rel" "$OUT/$rel"
  swift_only=$((swift_only + 1))
done < "$TMP/x86only.list"
[ "$swift_only" -eq 0 ] || echo "copied $swift_only x86_64-only Swift runtime dylib(s)"

# 3. merge Mach-O files
merged=0; kept=0
while IFS= read -r rel; do
  [ -f "$OUT/$rel" ] && [ ! -L "$OUT/$rel" ] || continue
  a="$(archs_of "$ARM/$rel")" || continue   # not Mach-O in the arm64 build
  b="$(archs_of "$X86/$rel")" || { echo "error: $rel is Mach-O ($a) in arm64 build but not in x86_64 build" >&2; exit 1; }
  if [ "$a" = "arm64" ] && [ "$b" = "x86_64" ]; then
    lipo -create "$ARM/$rel" "$X86/$rel" -output "$TMP/merged.bin"
    cat "$TMP/merged.bin" > "$OUT/$rel"   # keep mode and path of the copied file
    merged=$((merged + 1))
  elif [ "$a" = "$b" ] && [ "$a" = "arm64 x86_64" ]; then
    kept=$((kept + 1))                    # already universal on both sides; keep arm64-side copy
  else
    echo "error: cannot merge $rel: arm64 build has [$a], x86_64 build has [$b]" >&2
    exit 1
  fi
done < "$TMP/arm.list"
echo "merged $merged thin Mach-O file(s); kept $kept already-universal file(s)"

# 4. Info.plist
PLIST="$OUT/Contents/Info.plist"
[ -f "$PLIST" ] || { echo "error: $PLIST missing" >&2; exit 1; }
if $PB -c "Print :LSMinimumSystemVersion" "$PLIST" >/dev/null 2>&1; then
  $PB -c "Set :LSMinimumSystemVersion $MIN_X86_64" "$PLIST"
else
  $PB -c "Add :LSMinimumSystemVersion string $MIN_X86_64" "$PLIST"
fi
$PB -c "Delete :LSMinimumSystemVersionByArchitecture" "$PLIST" >/dev/null 2>&1 || true
$PB -c "Add :LSMinimumSystemVersionByArchitecture dict" "$PLIST"
$PB -c "Add :LSMinimumSystemVersionByArchitecture:arm64 string $MIN_ARM64" "$PLIST"
$PB -c "Add :LSMinimumSystemVersionByArchitecture:x86_64 string $MIN_X86_64" "$PLIST"
if [ -n "${HOPE_DESK_BUNDLE_VERSION:-}" ]; then
  if $PB -c "Print :CFBundleVersion" "$PLIST" >/dev/null 2>&1; then
    $PB -c "Set :CFBundleVersion $HOPE_DESK_BUNDLE_VERSION" "$PLIST"
  else
    $PB -c "Add :CFBundleVersion string $HOPE_DESK_BUNDLE_VERSION" "$PLIST"
  fi
fi

# 5. every Mach-O must be exactly arm64 + x86_64
bad=0
while IFS= read -r -d '' f; do
  r="$(archs_of "$f")" || continue
  if grep -qxF "./${f#$OUT/}" "$TMP/x86only.list"; then
    continue                              # x86_64-only Swift runtime, copied in step 2
  fi
  if [ "$r" != "arm64 x86_64" ]; then
    echo "error: ${f#$OUT/} is [$r], expected [arm64 x86_64]" >&2
    bad=1
  fi
done < <(find "$OUT" -type f -print0)
[ "$bad" -eq 0 ] || exit 1

# 6. ad-hoc sign
codesign --force --deep --sign - "$OUT"
codesign --verify --deep --strict "$OUT"
echo "universal bundle ready: $OUT"
