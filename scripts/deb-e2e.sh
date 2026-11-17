#!/bin/sh
# roadcase's .deb and apt repository, end to end, on Debian or Ubuntu as
# root (it installs packages: run it in a throwaway container):
#
# - packages scripts/linux-e2e (a tiny application linking roadcase-update)
#   as roadcase-fixture_1.0.0-1_<arch>.deb and _2.0.0-1, with
#   `roadcase release linux --format deb`, and checks what they hold;
# - makes a signed apt repository of 1.0.0 with `roadcase repo apt`, serves
#   it on 127.0.0.1, adds it as a deb822 source whose Signed-By is the
#   repository's key, and installs the application with apt;
# - publishes 2.0.0 to the feed and the repository: the installed 1.0.0
#   learns of it from the feed and refuses to download it
#   (ManagedBySystem apt), then `apt upgrade` installs it;
# - keeps one version, removes and purges the package.
#
#   scripts/deb-e2e.sh
#
# Needs cargo, dpkg-dev (dpkg-deb, dpkg-shlibdeps), binutils, python3 (to
# serve the repository) and curl. desktop-file-utils and shared-mime-info are
# checked when installed. Keeps the temporary folder with KEEP=1. From a Mac
# or any system with Docker, in a container of the host's architecture:
#
#   docker run --rm -v "$PWD":/src -v roadcase-target:/target -e CARGO_TARGET_DIR=/target \
#       -w /src rust:1.99.1-trixie sh -c \
#       'apt-get update -qq && apt-get install -y -qq python3 desktop-file-utils shared-mime-info >/dev/null &&
#        scripts/deb-e2e.sh'
set -eu
export DEBIAN_FRONTEND=noninteractive

[ "$(uname -s)" = Linux ] || { echo "deb-e2e: run this on Linux" >&2; exit 1; }
[ "$(id -u)" = 0 ] || { echo "deb-e2e: run this as root, in a throwaway container" >&2; exit 1; }

root=$(cd "$(dirname "$0")/.." && pwd)
fixture="$root/scripts/linux-e2e"
arch=$(uname -m)
debarch=$(dpkg --print-architecture)
port=${PORT:-8766}
work=$(mktemp -d)
chmod 755 "$work"
server=
cleanup() {
  [ -n "$server" ] && kill "$server" 2>/dev/null || true
  if [ "${KEEP:-0}" = 1 ]; then echo "kept $work"; else rm -rf "$work"; fi
}
trap cleanup EXIT INT TERM

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "ok: $*"; }

target=${CARGO_TARGET_DIR:-$root/target}
(cd "$root" && cargo build --quiet -p roadcase)
roadcase="$target/debug/roadcase"

cd "$work"
"$roadcase" keygen keys >/dev/null
"$roadcase" repo keygen repokeys --name "roadcase tests <tests@example.com>" >/dev/null
ROADCASE_REPO_SIGNING_KEY=$(cat repokeys/signing.key)
export ROADCASE_REPO_SIGNING_KEY
export FIXTURE_PUBLIC_KEY="$(tail -n 1 keys/update.pub)"
export FIXTURE_FEED="http://127.0.0.1:$port/feed/"
export FIXTURE_APT_URL="http://127.0.0.1:$port/apt"
export FIXTURE_REPO_KEY="$work/repokeys/key.gpg"
package=roadcase-fixture
mkdir -p www

# Two releases, each built and packaged.
for version in 1.0.0 2.0.0; do
  export FIXTURE_VERSION=$version
  (cd "$fixture" && CARGO_TARGET_DIR="$target/linux-e2e" cargo build --quiet --release)
  mkdir -p "bin-$version"
  cp "$target/linux-e2e/release/roadcase-fixture" "bin-$version/"
  export FIXTURE_BIN="$work/bin-$version/roadcase-fixture"
  deb=$("$roadcase" release linux --format deb --config "$fixture/roadcase.toml" \
    --out "$work/out-$version" --skip-build)
  [ "$(basename "$deb")" = "${package}_$version-1_$debarch.deb" ] || fail "unexpected name $(basename "$deb")"
  info=$(dpkg-deb --info "$deb")
  echo "$info" | grep -qx " Package: $package" || fail "no Package in $info"
  echo "$info" | grep -qx " Version: $version-1" || fail "no Version $version-1"
  echo "$info" | grep -qx " Architecture: $debarch" || fail "no Architecture $debarch"
  echo "$info" | grep -q "^ Depends: .*libc6 (>= " || fail "dpkg-shlibdeps added no libc6: $info"
  contents=$(dpkg-deb --contents "$deb")
  for path in ./opt/$package/bin/roadcase-fixture ./opt/$package/.roadcase-install \
      ./usr/share/applications/com.example.fixture.desktop \
      ./usr/share/icons/hicolor/64x64/apps/com.example.fixture.png \
      ./usr/share/icons/hicolor/scalable/apps/com.example.fixture.svg \
      ./usr/share/mime/packages/com.example.fixture.xml \
      ./etc/apt/sources.list.d/$package.sources \
      ./usr/share/keyrings/$package-archive-keyring.gpg; do
    echo "$contents" | grep -q " $path\$" || fail "$path is not in the .deb"
  done
  echo "$contents" | grep -q "^lrwxrwxrwx root/root .* ./usr/bin/$package -> /opt/$package/bin/roadcase-fixture\$" \
    || fail "no /usr/bin link owned by root: $(echo "$contents" | grep usr/bin)"
  [ "$(dpkg-deb --ctrl-tarfile "$deb" | tar -xO ./conffiles)" = "/etc/apt/sources.list.d/$package.sources" ] \
    || fail "the apt source is not a conffile"
  pass "$version packaged ($(basename "$deb")): layout, Depends from dpkg-shlibdeps, apt source as a conffile"
  if [ "$version" = 1.0.0 ]; then v1="$deb"; else v2="$deb"; fi
done

publish() { # version deb
  "$roadcase" publish --version "$1" --out www/feed --key keys/update.key \
    --notes "Release $1." --artifact "linux-$arch-deb=deb=$2" >/dev/null
  "$roadcase" repo apt www/apt --add "$2" --origin "roadcase tests" --label "roadcase tests" >/dev/null
}

publish 1.0.0 "$v1"
for path in www/apt/dists/stable/Release www/apt/dists/stable/Release.gpg www/apt/dists/stable/InRelease \
    "www/apt/dists/stable/main/binary-$debarch/Packages.gz" www/apt/key.gpg; do
  [ -f "$path" ] || fail "the repository has no $path"
done
pass "apt repository made: $(cd www/apt && find . -type f | sort | tr '\n' ' ')"

python3 -m http.server "$port" --bind 127.0.0.1 --directory www >/dev/null 2>&1 &
server=$!
for _ in $(seq 50); do
  curl -fs "$FIXTURE_FEED/latest.json" >/dev/null 2>&1 && break
  sleep 0.1
done

# The repository, added as its README says: the key in /usr/share/keyrings,
# a deb822 source naming it in Signed-By (the same file the .deb installs).
keyring=/usr/share/keyrings/$package-archive-keyring.gpg
sources=/etc/apt/sources.list.d/$package.sources
curl -fsS -o "$keyring" "$FIXTURE_APT_URL/key.gpg"
dpkg-deb --fsys-tarfile "$v1" | tar -xO ".$sources" > "$sources"
cat "$sources"
grep -qx "Signed-By: $keyring" "$sources" || fail "the source has no Signed-By"

update_output=$(apt-get update 2>&1) || fail "apt-get update: $update_output"
echo "$update_output" | grep -E "^(W|E):" && fail "apt-get update warned: $update_output"
apt-get install -y -q "$package" >/dev/null 2>&1 || fail "apt-get install $package"
[ "$(roadcase-fixture version)" = 1.0.0 ] || fail "the installed application is not 1.0.0"
[ "$(roadcase-fixture installed-by)" = deb ] || fail "the application does not know apt installed it"
[ "$(roadcase-fixture check)" = "up to date" ] || fail "1.0.0 found an update with 1.0.0 published"
listed=$(dpkg -L "$package")
for path in /opt/$package/bin/roadcase-fixture /opt/$package/.roadcase-install /usr/bin/$package \
    /usr/share/applications/com.example.fixture.desktop "$sources" "$keyring"; do
  echo "$listed" | grep -qx "$path" || fail "dpkg -L lists no $path"
done
[ "$(dpkg -S "$sources" | cut -d: -f1)" = "$package" ] || fail "the package does not own $sources"
dpkg-query -W -f='${Conffiles}\n' "$package" | grep -q " $sources " || fail "$sources is not a conffile"
if command -v desktop-file-validate >/dev/null; then
  desktop-file-validate /usr/share/applications/com.example.fixture.desktop || fail "desktop-file-validate"
fi
if command -v update-mime-database >/dev/null; then
  [ -f /usr/share/mime/application/x-roadcase-fixture.xml ] || fail "the shared-mime-info trigger did not run"
fi
pass "1.0.0 installed from the signed repository (Signed-By), its files listed, the source a conffile, the MIME trigger run"

# 2.0.0: announced by the feed, installed by apt.
publish 2.0.0 "$v2"
announced=$(roadcase-fixture check)
echo "$announced" | grep -qx "available 2.0.0 managed-by=deb" || fail "the feed's 2.0.0 is not announced: $announced"
echo "$announced" | grep -qx "download refused: ManagedBySystem apt" || fail "download was not refused: $announced"
[ ! -d "$HOME/.cache/roadcase-fixture/updates/2.0.0" ] || [ -z "$(ls -A "$HOME/.cache/roadcase-fixture/updates/2.0.0")" ] \
  || fail "something was downloaded"
pass "1.0.0 learns of 2.0.0 from the feed, and leaves it to apt (ManagedBySystem apt)"

apt-get update -q >/dev/null 2>&1 || fail "apt-get update"
policy=$(apt-cache policy "$package")
echo "$policy" | grep -q " 2.0.0-1 " && echo "$policy" | grep -q " 1.0.0-1 " || fail "both versions are not offered: $policy"
apt-get upgrade -y -q >/dev/null 2>&1 || fail "apt-get upgrade"
[ "$(dpkg-query -W -f='${Version}' "$package")" = 2.0.0-1 ] || fail "apt did not upgrade to 2.0.0-1"
[ "$(roadcase-fixture version)" = 2.0.0 ] || fail "the upgraded application is not 2.0.0"
[ "$(roadcase-fixture check)" = "up to date" ] || fail "2.0.0 still finds an update"
pass "apt upgrade installed 2.0.0-1, which is up to date"

# Keeping one version removes 1.0.0 from the pool and the index.
"$roadcase" repo apt www/apt --keep 1 --origin "roadcase tests" --label "roadcase tests" >/dev/null 2>&1
[ ! -e "www/apt/pool/main/r/$package/${package}_1.0.0-1_$debarch.deb" ] || fail "--keep 1 left 1.0.0 in the pool"
apt-get update -q >/dev/null 2>&1 || fail "apt-get update"
apt-cache policy "$package" | grep -q " 1.0.0-1 " && fail "1.0.0-1 is still offered"
pass "--keep 1 removed 1.0.0-1 from the pool and the index"

apt-get remove -y -q "$package" >/dev/null 2>&1 || fail "apt-get remove"
[ ! -e /opt/$package ] && [ ! -e /usr/bin/$package ] || fail "remove left the application"
[ -f "$sources" ] || fail "remove took the conffile, which only purge removes"
apt-get purge -y -q "$package" >/dev/null 2>&1 || fail "apt-get purge"
[ ! -e "$sources" ] || fail "purge left $sources"
pass "remove took the application, purge its apt source"

echo "deb-e2e: all passed"
