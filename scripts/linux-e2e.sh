#!/bin/sh
# roadcase on Linux, end to end: packages scripts/linux-e2e (a tiny application
# linking roadcase-update) as App-1.0.0-<arch>.AppImage and
# App-2.0.0-<arch>.AppImage, publishes them into a feed served on 127.0.0.1,
# and lets 1.0.0 update itself twice:
#
# - under its versioned name, it must become App-2.0.0-<arch>.AppImage beside
#   it, the old file removed, and start again from the new path, whose first
#   desktop integration points the menu entry at it;
# - under a stable name (App-<arch>.AppImage), it must be replaced in place in
#   one rename and start again from the same path.
#
# Both check the new file is executable and starts as 2.0.0. It also
# integrates the AppImage with the desktop under a temporary $HOME and removes
# it again.
#
# Run on Linux, natively (no cross-building), from anywhere:
#
#   scripts/linux-e2e.sh
#
# Needs cargo, python3 (to serve the feed), binutils, and sha256sum. Uses
# $APPIMAGETOOL and $APPIMAGE_RUNTIME when set; otherwise downloads
# appimagetool and the type-2 runtime for this architecture into the
# temporary folder, for this run only. Without /dev/fuse, AppImages run with
# APPIMAGE_EXTRACT_AND_RUN=1. desktop-file-utils and shared-mime-info are
# checked when installed. Keeps the temporary folder with KEEP=1.
set -eu

[ "$(uname -s)" = Linux ] || { echo "linux-e2e: run this on Linux" >&2; exit 1; }

root=$(cd "$(dirname "$0")/.." && pwd)
fixture="$root/scripts/linux-e2e"
arch=$(uname -m)
port=${PORT:-8765}
work=$(mktemp -d)
server=
cleanup() {
  [ -n "$server" ] && kill "$server" 2>/dev/null || true
  if [ "${KEEP:-0}" = 1 ]; then echo "kept $work"; else rm -rf "$work"; fi
}
trap cleanup EXIT INT TERM

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "ok: $*"; }

[ -e /dev/fuse ] || export APPIMAGE_EXTRACT_AND_RUN=1

# The tools, fetched for this run only when not given.
mkdir -p "$work/tools"
if [ -z "${APPIMAGETOOL:-}" ]; then
  APPIMAGETOOL="$work/tools/appimagetool"
  curl -fsSL -o "$APPIMAGETOOL" \
    "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$arch.AppImage"
  chmod +x "$APPIMAGETOOL"
fi
if [ -z "${APPIMAGE_RUNTIME:-}" ]; then
  APPIMAGE_RUNTIME="$work/tools/runtime-$arch"
  curl -fsSL -o "$APPIMAGE_RUNTIME" \
    "https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-$arch"
fi
export APPIMAGETOOL APPIMAGE_RUNTIME

target=${CARGO_TARGET_DIR:-$root/target}
(cd "$root" && cargo build --quiet -p roadcase)
roadcase="$target/debug/roadcase"

cd "$work"
"$roadcase" keygen keys >/dev/null
export FIXTURE_PUBLIC_KEY="$(tail -n 1 keys/update.pub)"
export FIXTURE_FEED="http://127.0.0.1:$port/"

# Two releases, each built, packaged and published in turn.
for version in 1.0.0 2.0.0; do
  export FIXTURE_VERSION=$version
  (cd "$fixture" && CARGO_TARGET_DIR="$target/linux-e2e" cargo build --quiet --release)
  mkdir -p "bin-$version"
  cp "$target/linux-e2e/release/roadcase-fixture" "bin-$version/"
  export FIXTURE_BIN="$work/bin-$version/roadcase-fixture"
  image=$("$roadcase" release linux --format appimage --config "$fixture/roadcase.toml" \
    --out "$work/out-$version" --skip-build)
  [ -x "$image" ] || fail "release linux made no AppImage ($image)"
  ls "$work/out-$version"/*.zip >/dev/null 2>&1 || fail "no symbols zip for $version"
  "$roadcase" publish --version "$version" --out feed --key keys/update.key \
    --notes "Release $version." --artifact "linux-$arch=appimage=$image" >/dev/null
  "$roadcase" verify --key keys/update.pub feed >/dev/null
  pass "$version packaged ($(basename "$image")) and published"
  [ "$(basename "$image")" = "App-$version-$arch.AppImage" ] || fail "unexpected name $(basename "$image")"
  if [ "$version" = 1.0.0 ]; then
    v1="$image"
  else
    v2="$image"
  fi
done

python3 -m http.server "$port" --bind 127.0.0.1 --directory feed >/dev/null 2>&1 &
server=$!
for _ in $(seq 50); do
  curl -fs "$FIXTURE_FEED/latest.json" >/dev/null 2>&1 && break
  sleep 0.1
done

mkdir -p Applications Stable
app="$work/Applications/App-1.0.0-$arch.AppImage"
new="$work/Applications/App-2.0.0-$arch.AppImage"
cp "$v1" "$app"
export HOME="$work/home" FIXTURE_LOG="$work/relaunched.log"
unset XDG_DATA_HOME
mkdir -p "$HOME"
data="$HOME/.local/share"
id=com.example.fixture

# Waits for the relaunched copy to log, and checks it is 2.0.0 from $1.
relaunched_from() {
  for _ in $(seq 100); do
    grep -q "^2.0.0 " "$FIXTURE_LOG" 2>/dev/null && break
    sleep 0.1
  done
  grep -qx "2.0.0 $1" "$FIXTURE_LOG" || fail "2.0.0 did not start again from $1: $(cat "$FIXTURE_LOG" 2>/dev/null)"
}

[ "$("$app" version)" = 1.0.0 ] || fail "the first AppImage is not 1.0.0"
pass "1.0.0 runs from its AppImage"

# Desktop integration.
[ "$("$app" desktop-install)" = changed=true ] || fail "desktop-install changed nothing"
entry="$data/applications/$id.desktop"
grep -qx "Exec=$app %u" "$entry" || fail "Exec is not the AppImage: $(grep ^Exec "$entry")"
grep -qx "TryExec=$app" "$entry" || fail "no TryExec of the AppImage"
grep -q '^MimeType=.*x-scheme-handler/roadcase-fixture;' "$entry" || fail "no URL scheme MIME type"
grep -q '^MimeType=.*application/x-roadcase-fixture;' "$entry" || fail "no document MIME type"
if command -v desktop-file-validate >/dev/null; then
  desktop-file-validate "$entry" || fail "desktop-file-validate"
fi
grep -q 'pattern="\*\.rcfix"' "$data/mime/packages/$id.xml" || fail "no glob in the MIME XML"
[ -f "$data/icons/hicolor/64x64/apps/$id.png" ] || fail "no 64x64 icon"
[ -f "$data/icons/hicolor/scalable/apps/$id.svg" ] || fail "no scalable icon"
if command -v update-desktop-database >/dev/null; then
  grep -q "x-scheme-handler/roadcase-fixture=$id.desktop" "$data/applications/mimeinfo.cache" \
    || fail "update-desktop-database did not run"
fi
if command -v update-mime-database >/dev/null; then
  [ -f "$data/mime/application/x-roadcase-fixture.xml" ] || fail "update-mime-database did not run"
fi
[ "$("$app" desktop-install)" = changed=false ] || fail "a second desktop-install changed something"
pass "desktop entry, MIME types and icons installed, caches refreshed, idempotent"
integrated=$(cd "$data" && find . -type f | sort)

# The update, under the versioned name: the new release takes its own name.
"$app" update | grep -q "installed, leaving" || fail "1.0.0 did not install the update"
relaunched_from "$new"
[ ! -e "$app" ] || fail "the old AppImage $app was not removed"
[ "$(stat -c %a "$new")" = 755 ] || fail "the AppImage is $(stat -c %a "$new"), not 755"
[ "$(sha256sum < "$new")" = "$(sha256sum < "$v2")" ] || fail "the AppImage is not 2.0.0's"
[ "$(ls -A Applications)" = "App-2.0.0-$arch.AppImage" ] || fail "Applications holds $(ls -A Applications)"
[ "$("$new" version)" = 2.0.0 ] || fail "the AppImage does not run as 2.0.0"
[ "$("$new" update)" = "up to date" ] || fail "2.0.0 still finds an update"
pass "App-1.0.0-$arch.AppImage became App-2.0.0-$arch.AppImage (mode 755), the old one removed, 2.0.0 started again from the new path"

# Its first start repoints the menu entry, and keeps everything else.
[ "$("$new" desktop-install)" = changed=true ] || fail "desktop-install from 2.0.0 changed nothing"
grep -qx "Exec=$new %u" "$entry" || fail "Exec is not the new AppImage: $(grep ^Exec "$entry")"
grep -qx "TryExec=$new" "$entry" || fail "TryExec is not the new AppImage: $(grep ^TryExec "$entry")"
[ "$(cd "$data" && find . -type f | sort)" = "$integrated" ] || fail "desktop-install added or removed files"
[ "$("$new" desktop-install)" = changed=false ] || fail "a second desktop-install from 2.0.0 changed something"
pass "the desktop entry follows the new file, nothing else added or removed"

[ "$("$new" desktop-remove)" = removed=true ] || fail "desktop-remove removed nothing"
[ ! -e "$entry" ] && [ ! -e "$data/mime/packages/$id.xml" ] || fail "desktop-remove left files"
[ -z "$(find "$data/icons" -name "$id.*")" ] || fail "desktop-remove left icons"
pass "desktop integration removed"

# The update, under a stable name: replaced in place.
stable="$work/Stable/App-$arch.AppImage"
cp "$v1" "$stable"
export FIXTURE_LOG="$work/relaunched-stable.log"
before=$(stat -c %i "$stable")
"$stable" update | grep -q "installed, leaving" || fail "1.0.0 (stable name) did not install the update"
relaunched_from "$stable"
[ "$(stat -c %i "$stable")" != "$before" ] || fail "the AppImage was written in place, not renamed over"
[ "$(stat -c %a "$stable")" = 755 ] || fail "the AppImage is $(stat -c %a "$stable"), not 755"
[ "$(sha256sum < "$stable")" = "$(sha256sum < "$v2")" ] || fail "the AppImage is not 2.0.0's"
[ "$(ls -A Stable)" = "App-$arch.AppImage" ] || fail "Stable holds $(ls -A Stable)"
[ "$("$stable" version)" = 2.0.0 ] || fail "the AppImage does not run as 2.0.0"
pass "App-$arch.AppImage replaced itself with 2.0.0 in one rename (mode 755) and 2.0.0 started again"

echo "linux-e2e: all passed"
