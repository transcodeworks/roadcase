#!/bin/bash
# roadcase on macOS, end to end: packages scripts/e2e-app (a tiny application
# linking roadcase-update, with a helper program that has entitlements of its
# own) as 1.0.0 and 2.0.0 with `roadcase release macos`, signed ad hoc, and
# checks each release:
#
# - the bundle passes `codesign --verify --deep --strict`, the application and
#   the helper each carry their own entitlements, and the helper its own
#   identifier;
# - each stripped program's UUID is its dSYM's, and the dSYMs are zipped;
# - the disk image verifies, is signed, and holds the application and a link
#   to /Applications;
# - the update archive holds no `._` files;
# - the feed folder (the archive, latest.json and its signature) verifies
#   with `roadcase verify`.
#
# Then it installs 1.0.0 from its disk image into a folder of the run's own,
# serves the feed on 127.0.0.1, and lets 1.0.0 update itself: with 1.0.0 in
# the feed it must say it is up to date; with 2.0.0 it must swap its bundle,
# start again as 2.0.0, and still pass `codesign`.
#
# With a "Developer ID Application" identity in the keychain that codesign
# can use without asking (and DEVELOPER_ID not 0), it also packages 3.0.0
# signed by it, and checks the hardened runtime, the secure timestamp, the
# team and the entitlements, and what Gatekeeper says. DEVELOPER_ID names
# the identity (a name or a SHA-1 hash) when there are several. That release
# is notarized only when NOTARY_PROFILE names a profile stored with
# `xcrun notarytool store-credentials`; without it, Gatekeeper must refuse
# the application as not notarized, and the script says so.
#
# Run on a Mac, from anywhere:
#
#   scripts/macos-e2e.sh
#
# Needs cargo, Xcode's command-line tools and python3 (to serve the feed).
# Builds into $CARGO_TARGET_DIR, or the repository's target/. PORT is the
# feed's port (8766). KEEP=1 keeps the temporary folder.
set -euo pipefail

[[ "$(uname -s)" == Darwin ]] || { echo "macos-e2e: run this on a Mac" >&2; exit 1; }

root="$(cd "$(dirname "$0")/.." && pwd)"
fixture="$root/scripts/e2e-app"
target="${CARGO_TARGET_DIR:-$root/target}"
arch="$(uname -m)"
[[ "$arch" == arm64 ]] && arch=aarch64
port="${PORT:-8766}"
work="$(mktemp -d)"
server=""
mounted=""
cleanup() {
  [[ -n "$server" ]] && kill "$server" 2>/dev/null || true
  [[ -n "$mounted" ]] && hdiutil detach "$mounted" -quiet 2>/dev/null || true
  if [[ "${KEEP:-0}" == 1 ]]; then echo "kept $work"; else rm -rf "$work"; fi
}
trap cleanup EXIT INT TERM

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "ok: $*"; }

name="Roadcase Fixture"
id="com.example.roadcase-fixture"

(cd "$root" && CARGO_TARGET_DIR="$target" cargo build --quiet -p roadcase)
roadcase="$target/debug/roadcase"

"$roadcase" keygen "$work/keys" >/dev/null
export FIXTURE_DIR="$fixture"
export FIXTURE_TARGET="$target/e2e-app"
export FIXTURE_FEED="http://127.0.0.1:$port/"
FIXTURE_PUBLIC_KEY="$(cat "$work/keys/update.pub")"
export FIXTURE_PUBLIC_KEY
ROADCASE_UPDATE_SECRET_KEY="$(cat "$work/keys/update.key")"
export ROADCASE_UPDATE_SECRET_KEY

uuids() { xcrun dwarfdump --uuid "$1" | awk '{ print $2, $3 }'; }
entitlements() { codesign --display --entitlements - --xml "$1" 2>/dev/null; }
signature() { codesign --display --verbose=2 "$1" 2>&1; }

# Packages one version with the identity in $CODESIGN_IDENTITY (ad hoc when
# unset) and checks what every release must hold.
release() {
  local version="$1" out="$work/out-$1" feed="$work/feed-$1"
  FIXTURE_VERSION="$version" FIXTURE_FEED_OUT="$feed" FIXTURE_NOTES="Release $version." \
    "$roadcase" release macos --config "${2:-$fixture/roadcase.toml}" --out "$out" >"$work/release-$version.log" 2>&1 ||
    { cat "$work/release-$version.log" >&2; fail "roadcase release macos ($version)"; }
  local app="$out/$name.app"
  local dmg="$out/$name-$version-$arch.dmg"
  local archive="$out/$name-$version-$arch.app.tar.gz"
  local symbols="$out/$name-$version-$arch.dSYMs.zip"

  codesign --verify --deep --strict "$app" || fail "$version: the bundle does not verify"
  [[ "$("$app/Contents/MacOS/fixture" version)" == "$version" ]] || fail "$version: the application is not $version"
  [[ "$("$app/Contents/MacOS/fixture" helper)" == "helper $version" ]] || fail "$version: the helper does not run"
  [[ -f "$app/Contents/Resources/docs/guide.md" ]] || fail "$version: the resource is missing"
  [[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")" == "$version" ]] ||
    fail "$version: Info.plist has another version"
  pass "$version: the bundle is signed, verifies, and runs with its helper"

  # Captured first: under pipefail, a grep -q that stops reading fails the pipe.
  local app_entitlements helper_entitlements
  app_entitlements="$(entitlements "$app")"
  helper_entitlements="$(entitlements "$app/Contents/MacOS/fixture-helper")"
  grep -q com.apple.security.network.client <<<"$app_entitlements" ||
    fail "$version: the application lacks its entitlements"
  if grep -q disable-library-validation <<<"$app_entitlements"; then
    fail "$version: the application carries the helper's entitlements"
  fi
  grep -q com.apple.security.cs.disable-library-validation <<<"$helper_entitlements" ||
    fail "$version: the helper lacks its entitlements"
  if grep -q network.client <<<"$helper_entitlements"; then
    fail "$version: the helper carries the application's entitlements"
  fi
  grep -q "^Identifier=$id.helper$" <<<"$(signature "$app/Contents/MacOS/fixture-helper")" ||
    fail "$version: the helper is not signed as $id.helper"
  grep -q "^Identifier=$id$" <<<"$(signature "$app")" ||
    fail "$version: the bundle is not signed as $id"
  pass "$version: the application and the helper each have their own entitlements and identifier"

  for program in fixture fixture-helper; do
    local dsym="$out/$name.dSYMs/$program.dSYM"
    [[ -d "$dsym" ]] || fail "$version: no dSYM for $program"
    local uuid
    uuid="$(uuids "$app/Contents/MacOS/$program")"
    [[ -n "$uuid" && "$uuid" == "$(uuids "$dsym")" ]] || fail "$version: $program's dSYM has another UUID"
    # Stripped: the symbol table no longer names main.
    if grep -q ' T _main$' <<<"$(nm "$app/Contents/MacOS/$program" 2>/dev/null)"; then
      fail "$version: $program is not stripped"
    fi
  done
  if [[ -n "$(find "$app" -name '*.dSYM')" ]]; then fail "$version: a dSYM is inside the bundle"; fi
  grep -q 'fixture-helper.dSYM/Contents/Info.plist' <<<"$(unzip -l "$symbols")" ||
    fail "$version: the symbols zip lacks a dSYM"
  pass "$version: dSYMs match their stripped programs by UUID, and are zipped"

  hdiutil verify -quiet "$dmg" || fail "$version: the disk image does not verify"
  codesign --verify "$dmg" || fail "$version: the disk image is not signed"
  mounted="$work/mount-$version"
  hdiutil attach "$dmg" -nobrowse -readonly -quiet -mountpoint "$mounted" || fail "$version: the disk image does not mount"
  [[ -x "$mounted/$name.app/Contents/MacOS/fixture" ]] || fail "$version: the disk image lacks the application"
  [[ "$(readlink "$mounted/Applications")" == /Applications ]] || fail "$version: the disk image lacks the Applications link"
  codesign --verify --deep --strict "$mounted/$name.app" || fail "$version: the application in the disk image does not verify"
  if [[ "$version" == 1.0.0 ]]; then
    mkdir -p "$work/Applications"
    ditto "$mounted/$name.app" "$work/Applications/$name.app"
  fi
  hdiutil detach "$mounted" -quiet
  mounted=""
  pass "$version: the disk image verifies, is signed, and holds the application and the Applications link"

  local listing
  listing="$(tar -tzf "$archive")"
  if grep -Eq '(^|/)\._' <<<"$listing"; then fail "$version: the update archive holds ._ files"; fi
  grep -q "^$name.app/Contents/_CodeSignature/CodeResources$" <<<"$listing" ||
    fail "$version: the update archive lacks the signed bundle"
  "$roadcase" verify --key "$work/keys/update.pub" "$feed" >"$work/verify-$version.log" ||
    { cat "$work/verify-$version.log" >&2; fail "$version: the feed does not verify"; }
  grep -q "macos-$arch: $name-$version-$arch.app.tar.gz matches" "$work/verify-$version.log" ||
    fail "$version: the feed lacks the update archive"
  grep -q "\"notes\": \"Release $version.\"" "$feed/latest.json" || fail "$version: the feed lacks the notes"
  [[ ! -e "$feed/$name-$version-$arch.dmg" ]] || fail "$version: the disk image is in the feed"
  pass "$version: the update archive has no ._ files, and the feed verifies"
}

unset CODESIGN_IDENTITY CODESIGN_KEYCHAIN CODESIGN_CERTIFICATE NOTARY_APPLE_ID NOTARY_KEY
release 1.0.0
release 2.0.0
grep -q '^Signature=adhoc$' <<<"$(signature "$work/out-1.0.0/$name.app")" ||
  fail "1.0.0 is not signed ad hoc"
pass "both releases are signed ad hoc"

# The feed, served as a host would serve it.
mkdir "$work/served"
cp "$work/feed-1.0.0/"* "$work/served/"
python3 -m http.server "$port" --bind 127.0.0.1 --directory "$work/served" >"$work/server.log" 2>&1 &
server=$!
for _ in $(seq 1 50); do
  curl -fsS "http://127.0.0.1:$port/latest.json" >/dev/null 2>&1 && break
  sleep 0.1
done
curl -fsS "http://127.0.0.1:$port/latest.json" >/dev/null || fail "the feed is not served on port $port"

installed="$work/Applications/$name.app"
export FIXTURE_STAGING="$work/staging"
[[ "$("$installed/Contents/MacOS/fixture" update)" == "up to date" ]] || fail "1.0.0 found an update in its own release"
pass "1.0.0 is up to date with 1.0.0 in the feed"

cp "$work/feed-2.0.0/"* "$work/served/"
"$installed/Contents/MacOS/fixture" update >"$work/update.log" 2>&1 ||
  { cat "$work/update.log" >&2; fail "1.0.0 could not install 2.0.0"; }
grep -q "available 2.0.0" "$work/update.log" || fail "1.0.0 did not hear of 2.0.0"
# The swap waits for 1.0.0 to leave, then opens 2.0.0, which writes the log.
for _ in $(seq 1 150); do
  [[ -f "$work/Applications/fixture.log" ]] && break
  sleep 0.2
done
[[ "$(cat "$work/Applications/fixture.log" 2>/dev/null)" == 2.0.0 ]] ||
  fail "2.0.0 did not start again after the update ($(cat "$work/Applications/fixture.log" 2>/dev/null))"
[[ "$("$installed/Contents/MacOS/fixture" version)" == 2.0.0 ]] || fail "the installed bundle is not 2.0.0"
[[ "$("$installed/Contents/MacOS/fixture" helper)" == "helper 2.0.0" ]] || fail "the installed helper is not 2.0.0"
codesign --verify --deep --strict "$installed" || fail "the updated bundle does not verify"
leftovers="$(find "$work/Applications" -mindepth 1 -maxdepth 1 ! -name "$name.app" ! -name fixture.log)"
[[ -z "$leftovers" ]] || fail "the update left $leftovers"
[[ "$("$installed/Contents/MacOS/fixture" update)" == "up to date" ]] || fail "2.0.0 found an update in its own release"
pass "1.0.0 updated itself to 2.0.0 by swapping its bundle, and started again"

# A Developer ID, when this Mac has one that codesign can use unasked.
developer="${DEVELOPER_ID:-}"
if [[ "$developer" == 0 ]]; then
  echo "skipped: the Developer ID checks (DEVELOPER_ID=0)"
  exit 0
fi
if [[ -z "$developer" ]]; then
  identities="$(security find-identity -v -p codesigning)"
  developer="$(awk '/"Developer ID Application:/ { print $2; exit }' <<<"$identities")"
fi
if [[ -z "$developer" ]]; then
  echo "skipped: the Developer ID checks (no Developer ID Application identity in the keychain)"
  exit 0
fi
# codesign asks in a window when the keychain does not let it use the key:
# give it twenty seconds on a throwaway file.
cp /usr/bin/true "$work/probe"
if ! perl -e 'alarm 20; exec @ARGV' codesign --force --sign "$developer" "$work/probe" >"$work/probe.log" 2>&1; then
  echo "skipped: the Developer ID checks (codesign cannot use the identity without asking: $(tail -n 1 "$work/probe.log"))"
  exit 0
fi

config="$fixture/roadcase.toml"
if [[ -n "${NOTARY_PROFILE:-}" ]]; then
  # The same config, in the fixture's own folder's terms, with notarization.
  config="$work/roadcase.toml"
  { cat "$fixture/roadcase.toml"; printf '\n[macos.notarize]\n'; } >"$config"
fi
export CODESIGN_IDENTITY="$developer"
release 3.0.0 "$config"
app="$work/out-3.0.0/$name.app"
signed="$(signature "$app")"
grep -q '^Authority=Developer ID Application:' <<<"$signed" || fail "3.0.0 is not signed by the Developer ID"
grep -q '^Authority=Developer ID Certification Authority$' <<<"$signed" || fail "3.0.0's certificate chain is not Apple's"
grep -Eq '^CodeDirectory .*flags=0x10000\(runtime\)' <<<"$signed" || fail "3.0.0 has no hardened runtime"
grep -q '^Timestamp=' <<<"$signed" || fail "3.0.0 has no secure timestamp"
grep -Eq '^TeamIdentifier=[A-Z0-9]{10}$' <<<"$signed" || fail "3.0.0 names no team"
helper="$(signature "$app/Contents/MacOS/fixture-helper")"
grep -Eq '^CodeDirectory .*flags=0x10000\(runtime\)' <<<"$helper" || fail "3.0.0's helper has no hardened runtime"
grep -q '^Timestamp=' <<<"$helper" || fail "3.0.0's helper has no secure timestamp"
team="$(sed -n 's/^TeamIdentifier=//p' <<<"$signed")"
codesign --verify --deep --strict -R="anchor apple generic and certificate leaf[subject.OU] = \"$team\"" "$app" ||
  fail "3.0.0 does not meet the requirement the updater checks"
grep -q '^Authority=Developer ID Application:' <<<"$(signature "$work/out-3.0.0/$name-3.0.0-$arch.dmg")" ||
  fail "3.0.0's disk image is not signed by the Developer ID"
pass "3.0.0 is signed by the Developer ID: hardened runtime, secure timestamp, team, per-program entitlements"

if [[ -n "${NOTARY_PROFILE:-}" ]]; then
  xcrun stapler validate "$app" >/dev/null || fail "3.0.0 has no stapled ticket"
  xcrun stapler validate "$work/out-3.0.0/$name-3.0.0-$arch.dmg" >/dev/null || fail "3.0.0's disk image has no stapled ticket"
  spctl --assess --type execute "$app" || fail "Gatekeeper refuses the notarized 3.0.0"
  mkdir "$work/staging-3"
  tar -xzf "$work/out-3.0.0/$name-3.0.0-$arch.app.tar.gz" -C "$work/staging-3"
  xcrun stapler validate "$work/staging-3/$name.app" >/dev/null || fail "the update archive's bundle has no stapled ticket"
  pass "3.0.0 is notarized and stapled: the application, the disk image, and the bundle in the update archive"
else
  verdict="$(spctl --assess --type execute --verbose=4 "$app" 2>&1 || true)"
  if grep -q 'accepted' <<<"$verdict"; then
    fail "Gatekeeper accepted an application that was not notarized: $verdict"
  fi
  grep -q 'Unnotarized Developer ID' <<<"$verdict" ||
    fail "Gatekeeper refused 3.0.0 for another reason than notarization: $verdict"
  pass "3.0.0 is not notarized (no NOTARY_PROFILE), and Gatekeeper refuses it for that reason alone"
fi
