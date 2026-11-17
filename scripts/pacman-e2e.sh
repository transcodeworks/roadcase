#!/bin/sh
# roadcase's pacman package, pacman repository and AUR -bin package, end to
# end, on Arch Linux as root (it installs packages: run it in a throwaway
# container):
#
# - packages scripts/linux-e2e (a tiny application linking roadcase-update)
#   as roadcase-fixture-1.0.0-1-<arch>.pkg.tar.zst and 2.0.0-1, with
#   `roadcase release linux --format pacman` (makepkg, as a user other than
#   root), and checks what they hold;
# - makes a signed pacman repository of 1.0.0 with `roadcase repo pacman`,
#   serves it on 127.0.0.1, trusts its key with pacman-key, and installs the
#   application with `pacman -Sy`;
# - publishes 2.0.0 to the feed and the repository: the installed 1.0.0
#   learns of it from the feed and refuses to download it
#   (ManagedBySystem pacman), then `pacman -Syu` installs it;
# - writes the AUR -bin package with `roadcase repo aur`, checks its .SRCINFO
#   against `makepkg --printsrcinfo`, builds it with makepkg (downloading
#   the package from the repository and checking its SHA-256) and installs
#   it in place of the repository's package.
#
#   scripts/pacman-e2e.sh
#
# Needs base-devel, rust (cargo), python3 (to serve the repository) and curl;
# desktop-file-utils and shared-mime-info are checked when installed. Keeps
# the temporary folder with KEEP=1. On an x86_64 Linux runner, or a Mac or
# any system with Docker:
#
#   docker run --rm -v "$PWD":/src -v roadcase-target:/target -e CARGO_TARGET_DIR=/target \
#       -w /src archlinux:latest sh -c \
#       'pacman -Syu --noconfirm --needed base-devel rust python desktop-file-utils shared-mime-info >/dev/null &&
#        scripts/pacman-e2e.sh'
#
# The official Arch Linux image is x86_64 only. On another architecture it
# runs emulated (--platform linux/amd64), where compiling is very slow, so
# the programs the test runs can be built beforehand, in a container of the
# host's architecture, for x86_64:
#
#   scripts/pacman-e2e.sh prebuild <folder>   # cargo with the x86_64-unknown-linux-gnu target
#
# and given to the test with PREBUILT=<folder>. Only the test's own programs
# (roadcase and the fixture) are built so; the packages are made, served and
# installed on Arch Linux, by makepkg and pacman. Both runs must use the same
# PORT, which the fixture holds. qemu cannot run pacman's download sandbox,
# so the emulated container disables it. From an aarch64 Mac:
#
#   docker run --rm -v "$PWD":/src -v roadcase-target:/target -v roadcase-prebuilt:/prebuilt \
#       -e CARGO_TARGET_DIR=/target -w /src rust:1.99.1-trixie sh -c \
#       'apt-get update -qq && apt-get install -y -qq gcc-x86-64-linux-gnu libc6-dev-amd64-cross >/dev/null &&
#        rustup target add x86_64-unknown-linux-gnu &&
#        CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc \
#        scripts/pacman-e2e.sh prebuild /prebuilt'
#   docker run --rm --platform linux/amd64 -v "$PWD":/src:ro -v roadcase-prebuilt:/prebuilt:ro \
#       -e PREBUILT=/prebuilt -w /src archlinux:latest sh -c \
#       "sed -i '/^\[options\]/a DisableSandbox' /etc/pacman.conf &&
#        pacman -Syu --noconfirm --needed base-devel python desktop-file-utils shared-mime-info >/dev/null &&
#        scripts/pacman-e2e.sh"
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
fixture="$root/scripts/linux-e2e"
port=${PORT:-8767}
target=${CARGO_TARGET_DIR:-$root/target}
export FIXTURE_FEED="http://127.0.0.1:$port/feed/"

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "ok: $*"; }

# The programs, for x86_64, with the update key they hold.
if [ "${1:-}" = prebuild ]; then
  out=${2:?usage: pacman-e2e.sh prebuild <folder>}
  triple=x86_64-unknown-linux-gnu
  mkdir -p "$out"
  (cd "$root" && cargo build --quiet -p roadcase && cargo build --quiet -p roadcase --target $triple)
  rm -rf "$out/keys"
  "$target/debug/roadcase" keygen "$out/keys" >/dev/null
  cp "$target/$triple/debug/roadcase" "$out/roadcase"
  export FIXTURE_PUBLIC_KEY="$(tail -n 1 "$out/keys/update.pub")"
  for version in 1.0.0 2.0.0; do
    (cd "$fixture" && FIXTURE_VERSION=$version CARGO_TARGET_DIR="$target/linux-e2e" \
      cargo build --quiet --release --target $triple)
    cp "$target/linux-e2e/$triple/release/roadcase-fixture" "$out/roadcase-fixture-$version"
  done
  echo "pacman-e2e: built $(ls "$out" | tr '\n' ' ')in $out for PORT=$port"
  exit 0
fi

[ "$(uname -s)" = Linux ] && [ -f /etc/arch-release ] || { echo "pacman-e2e: run this on Arch Linux" >&2; exit 1; }
[ "$(id -u)" = 0 ] || { echo "pacman-e2e: run this as root, in a throwaway container" >&2; exit 1; }

arch=$(uname -m)
work=$(mktemp -d)
server=
cleanup() {
  [ -n "$server" ] && kill "$server" 2>/dev/null || true
  if [ "${KEEP:-0}" = 1 ]; then echo "kept $work"; else rm -rf "$work"; fi
}
trap cleanup EXIT INT TERM

# makepkg refuses root: packages are made by a user of their own.
id builder >/dev/null 2>&1 || useradd -m builder
chown builder "$work"
chmod 755 "$work"
as_builder() { runuser -u builder -- env HOME=/home/builder "$@"; }

cd "$work"
if [ -n "${PREBUILT:-}" ]; then
  roadcase="$work/roadcase"
  cp "$PREBUILT/roadcase" "$roadcase"
  cp -r "$PREBUILT/keys" keys
  for version in 1.0.0 2.0.0; do
    mkdir -p "bin-$version"
    cp "$PREBUILT/roadcase-fixture-$version" "bin-$version/roadcase-fixture"
  done
else
  (cd "$root" && cargo build --quiet -p roadcase)
  roadcase="$target/debug/roadcase"
  "$roadcase" keygen keys >/dev/null
  export FIXTURE_PUBLIC_KEY="$(tail -n 1 keys/update.pub)"
  for version in 1.0.0 2.0.0; do
    (cd "$fixture" && FIXTURE_VERSION=$version CARGO_TARGET_DIR="$target/linux-e2e" cargo build --quiet --release)
    mkdir -p "bin-$version"
    cp "$target/linux-e2e/release/roadcase-fixture" "bin-$version/"
  done
fi
chown -R builder "$work"
package=roadcase-fixture

# Two releases, each packaged by makepkg.
for version in 1.0.0 2.0.0; do
  pkg=$(as_builder FIXTURE_VERSION=$version FIXTURE_BIN="$work/bin-$version/roadcase-fixture" \
    "$roadcase" release linux --format pacman --config "$fixture/roadcase.toml" \
    --out "$work/out-$version" --skip-build)
  [ "$(basename "$pkg")" = "$package-$version-1-$arch.pkg.tar.zst" ] || fail "unexpected name $(basename "$pkg")"
  info=$(pacman -Qip "$pkg")
  echo "$info" | grep -q "^Version *: $version-1\$" || fail "no version $version-1: $info"
  echo "$info" | grep -q "^Architecture *: $arch\$" || fail "no architecture $arch"
  echo "$info" | grep -q "^Licenses *: MIT\$" || fail "no licence"
  contents=$(bsdtar -tvf "$pkg")
  for path in .PKGINFO .MTREE .BUILDINFO opt/$package/bin/roadcase-fixture opt/$package/.roadcase-install \
      usr/share/applications/com.example.fixture.desktop \
      usr/share/icons/hicolor/64x64/apps/com.example.fixture.png \
      usr/share/mime/packages/com.example.fixture.xml; do
    echo "$contents" | grep -q " $path\$" || fail "$path is not in the package"
  done
  echo "$contents" | grep -q "root *root .* usr/bin/$package -> /opt/$package/bin/roadcase-fixture\$" \
    || fail "no /usr/bin link owned by root: $(echo "$contents" | grep usr/bin)"
  echo "$contents" | grep -q "etc/apt" && fail "the pacman package holds an apt source"
  pass "$version packaged by makepkg ($(basename "$pkg")), with .PKGINFO, .MTREE and the layout"
  if [ "$version" = 1.0.0 ]; then v1="$pkg"; else v2="$pkg"; fi
done

"$roadcase" repo keygen repokeys --name "roadcase tests <tests@example.com>" > fingerprint
ROADCASE_REPO_SIGNING_KEY=$(cat repokeys/signing.key)
export ROADCASE_REPO_SIGNING_KEY
mkdir -p www
publish() { # version package
  "$roadcase" publish --version "$1" --out www/feed --key keys/update.key \
    --notes "Release $1." --artifact "linux-$arch-pacman=pacman=$2" >/dev/null
  "$roadcase" repo pacman www/arch --name fixture --add "$2" >/dev/null
}
publish 1.0.0 "$v1"
for path in fixture.db fixture.db.sig fixture.db.tar.gz fixture.files fixture.files.sig \
    "$(basename "$v1")" "$(basename "$v1").sig"; do
  [ -f "www/arch/$arch/$path" ] || fail "the repository has no $arch/$path"
done
pass "pacman repository made: $(cd www/arch && find . -type f | sort | tr '\n' ' ')"

python3 -m http.server "$port" --bind 127.0.0.1 --directory www >/dev/null 2>&1 &
server=$!
for _ in $(seq 100); do
  curl -fs "$FIXTURE_FEED/latest.json" >/dev/null 2>&1 && break
  sleep 0.1
done

# The repository, added as its README says: its key trusted, then a section.
pacman-key --init >/dev/null 2>&1
pacman-key --add www/arch/key.asc >/dev/null 2>&1 || fail "pacman-key --add"
pacman-key --lsign-key "$(cat fingerprint)" >/dev/null 2>&1 || fail "pacman-key --lsign-key"
cat >> /etc/pacman.conf <<EOF

[fixture]
SigLevel = Required DatabaseRequired
Server = http://127.0.0.1:$port/arch/\$arch
EOF
pacman -Sy --noconfirm "$package" >/dev/null 2>&1 || fail "pacman -Sy $package: $(pacman -Sy --noconfirm "$package" 2>&1 | tail -5)"
[ "$(roadcase-fixture version)" = 1.0.0 ] || fail "the installed application is not 1.0.0"
[ "$(roadcase-fixture installed-by)" = pacman ] || fail "the application does not know pacman installed it"
[ "$(roadcase-fixture check)" = "up to date" ] || fail "1.0.0 found an update with 1.0.0 published"
listed=$(pacman -Qlq "$package")
for path in /opt/$package/bin/roadcase-fixture /opt/$package/.roadcase-install /usr/bin/$package \
    /usr/share/applications/com.example.fixture.desktop; do
  echo "$listed" | grep -qx "$path" || fail "pacman -Ql lists no $path"
done
pacman -Qkk "$package" >/dev/null 2>&1 || fail "pacman -Qkk finds the files changed: $(pacman -Qkk "$package" 2>&1 | tail -3)"
if command -v desktop-file-validate >/dev/null; then
  desktop-file-validate /usr/share/applications/com.example.fixture.desktop || fail "desktop-file-validate"
fi
if command -v update-mime-database >/dev/null; then
  [ -f /usr/share/mime/application/x-roadcase-fixture.xml ] || fail "the shared-mime-info hook did not run"
fi
pass "1.0.0 installed from the signed repository (SigLevel Required), its files as packaged, the MIME hook run"

# A database signed by another key is refused.
cp -r www/arch www/arch.good
"$roadcase" repo keygen otherkeys --name "someone else <else@example.com>" >/dev/null
"$roadcase" repo pacman www/arch --name fixture --key otherkeys/signing.key >/dev/null 2>&1
pacman -Sy --noconfirm >/dev/null 2>&1 && fail "pacman accepted a database signed by an unknown key"
rm -rf www/arch && mv www/arch.good www/arch
pass "a database signed by another key is refused"

# 2.0.0: announced by the feed, installed by pacman.
publish 2.0.0 "$v2"
announced=$(roadcase-fixture check)
echo "$announced" | grep -qx "available 2.0.0 managed-by=pacman" || fail "the feed's 2.0.0 is not announced: $announced"
echo "$announced" | grep -qx "download refused: ManagedBySystem pacman" || fail "download was not refused: $announced"
pass "1.0.0 learns of 2.0.0 from the feed, and leaves it to pacman (ManagedBySystem pacman)"

pacman -Syu --noconfirm >/dev/null 2>&1 || fail "pacman -Syu: $(pacman -Syu --noconfirm 2>&1 | tail -5)"
[ "$(pacman -Q "$package")" = "$package 2.0.0-1" ] || fail "pacman did not upgrade to 2.0.0-1: $(pacman -Q "$package")"
[ "$(roadcase-fixture version)" = 2.0.0 ] || fail "the upgraded application is not 2.0.0"
[ "$(roadcase-fixture check)" = "up to date" ] || fail "2.0.0 still finds an update"
[ -f "www/arch/$arch/$(basename "$v1")" ] || fail "the repository no longer keeps 1.0.0"
pass "pacman -Syu installed 2.0.0-1, which is up to date; 1.0.0 stays in the repository"

# The AUR -bin package, built by makepkg from the published package.
"$roadcase" repo aur "$v2" --url "http://127.0.0.1:$port/arch" --out aur \
  --maintainer "roadcase tests <tests@example.com>" >/dev/null
chown -R builder aur
(cd aur && as_builder makepkg --printsrcinfo) > srcinfo.makepkg
diff -u srcinfo.makepkg aur/.SRCINFO || fail ".SRCINFO differs from makepkg --printsrcinfo"
(cd aur && as_builder makepkg --force --noconfirm --noprogressbar >/dev/null 2>&1) \
  || fail "makepkg of the -bin package: $(cd aur && as_builder makepkg --force --noconfirm 2>&1 | tail -5)"
bin=$(ls aur/$package-bin-2.0.0-1-$arch.pkg.tar.zst) || fail "makepkg made no -bin package"
pacman -R --noconfirm "$package" >/dev/null 2>&1 || fail "pacman -R"
pacman -U --noconfirm "$bin" >/dev/null 2>&1 || fail "pacman -U $bin"
[ "$(pacman -Qoq /usr/bin/$package)" = "$package-bin" ] || fail "/usr/bin/$package is not the -bin package's"
[ "$(roadcase-fixture version)" = 2.0.0 ] || fail "the -bin package's application is not 2.0.0"
[ "$(roadcase-fixture installed-by)" = pacman ] || fail "the -bin package's copy does not know pacman installed it"
pacman -Qkk "$package-bin" >/dev/null 2>&1 || fail "pacman -Qkk $package-bin"
pass "the AUR -bin package: .SRCINFO as makepkg writes it, built from the published package, installed"

pacman -R --noconfirm "$package-bin" >/dev/null 2>&1 || fail "pacman -R $package-bin"
[ ! -e /opt/$package ] && [ ! -e /usr/bin/$package ] || fail "remove left the application"
pass "removed"

echo "pacman-e2e: all passed"
