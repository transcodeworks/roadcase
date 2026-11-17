# roadcase

Package, sign and update desktop applications written in Rust.

You describe your application once, in a `roadcase.toml`. roadcase turns your build into
a signed, notarized macOS app and disk image, a signed Windows installer, or a Linux
AppImage, `.deb` and pacman package, publishes it as a signed update, keeps the signed apt
and pacman repositories the Linux packages update from, and gives your application a small
library to find, download and install that update.

> **Status: early.** Packaging, publishing and updating work on all three systems, each
> checked end to end by a script in `scripts/`: macOS (signed bundle, dSYMs, disk image,
> update archive, a bundle-swap update) and Linux (AppImage, `.deb`, pacman package, their
> repositories) on real machines, Windows (NSIS installer, silent update, uninstall) in
> CI. Notarization follows scripts that have shipped notarized applications, and has not
> yet been run from roadcase itself. See the
> [status table](crates/roadcase-bundle#status) for what was verified where. The crates
> are not on crates.io yet.

## Why roadcase

Most packagers assume an application is one executable. roadcase is for applications
that are more than that:

- **Several programs in one app.** A helper process, a command-line tool, a third-party
  binary: each is signed with its own entitlements and identifier, inside out, the way
  Apple requires.
- **App extensions and launch agents.** A File Provider or other `.appex` goes in
  `Contents/PlugIns`, a launch agent in `Contents/Library/LaunchAgents`, and both are
  signed.
- **Everything notarized.** The app and the disk image are both notarized and stapled.
- **Symbols kept, binaries stripped.** Debug symbols go into dSYMs on macOS and
  `.debug` files on Linux, for your crash reporter; what ships is small.
- **A Linux AppImage that behaves like an installed app.** Helper programs beside the
  executable, a desktop entry, hicolor icons, your URL schemes and document types as
  MIME types, and one call at startup that registers them with the desktop.
- **Linux packages the conventional way.** A `.deb` and a pacman package install under
  `/opt/<package>` with a link in `/usr/bin`, a desktop entry, icons and MIME types, and
  are updated by apt and pacman from signed repositories roadcase keeps as static folders
  (or, on Arch, from the AUR). Your application learns that a release exists, and tells
  the person to update with their package manager; it never installs a package itself.
- **Your own installer.** On Windows, roadcase runs your NSIS script, with the registry
  entries for URL schemes and file types, and signs every executable with the command
  you give it (Azure Artifact Signing, `signtool`, or anything else).
- **Updates you control.** No update server, no account: a release is a folder of static
  files on any HTTPS host, signed with a key only you hold. Your application decides
  when to offer and when to install, and can bring its own HTTP transport, for example to
  send a token to a feed that only answers signed-in users.
- **Native builds.** macOS is packaged on a Mac, Windows on Windows, and Linux on Linux,
  each architecture on its own. Nothing is downloaded while packaging; the tools come
  from the machine.

If your application is a single executable and you want one tool for every system,
[cargo-packager](https://github.com/crabnebula-dev/cargo-packager) or
[Velopack](https://velopack.io) may suit you better.

## Platforms

| | macOS | Windows | Linux AppImage | Linux `.deb` (Debian, Ubuntu) | Linux pacman package (Arch) |
| --- | --- | --- | --- | --- | --- |
| Packaging | `.app`, `.dmg` | NSIS installer | AppImage | `dpkg-deb` | `makepkg`, on Arch |
| Distributed from | your HTTPS host | your HTTPS host | your HTTPS host | a signed apt repository | a signed pacman repository, or the AUR (`-bin`) |
| Updates | the signed `.app` is swapped in place | the installer runs silently | the AppImage replaces its own file | `apt upgrade`; the application only announces the release | `pacman -Syu` or an AUR helper; the application only announces the release |
| URL schemes, file types | Info.plist | registry, by the installer | desktop entry and MIME types, registered by the application | desktop entry and MIME types, installed by the package | desktop entry and MIME types, installed by the package |

Linux is x86_64 and aarch64, each packaged on its own architecture.

## How it works

```
  roadcase.toml
       │
       ▼
  roadcase release macos         build ─► assemble .app ─► dSYMs + strip ─► sign ─► notarize ─► .dmg, .app.tar.gz
  roadcase release windows       build ─► stage ─► sign ─► NSIS installer ─► sign
  roadcase release linux         build ─► assemble AppDir ─► .debug + strip ─► AppImage, .deb, pacman package
       │
       ▼                         each release ends by writing its part of the feed:
  feed folder                    latest.json + its signature + the artifacts
       │                         (upload the folder to any HTTPS host; `roadcase publish` does it by hand)
  roadcase repo apt | pacman     the signed apt or pacman repository folder (upload it too)
  roadcase repo aur              the AUR -bin PKGBUILD and .SRCINFO (git push them to the AUR)
       │
       ▼
  your application               roadcase-update: check ─► download ─► verify ─► install
                                 (a .deb or pacman copy: check, then "update with apt/pacman")
```

Each release is signed with your update key. Every installed copy holds the public half,
so it only accepts what you signed, and never an older version.

## Getting started

### 1. Install the tool

Each [release](https://github.com/transcodeworks/roadcase/releases) carries the `roadcase`
program, built on its own system: macOS (Apple silicon and Intel, signed and notarized),
Windows x86_64 (signed) and Linux x86_64 and aarch64 (statically linked, so any
distribution runs it). Each is `roadcase-<target>.tar.gz` (`.zip` on Windows) beside its
`.sha256`, with a build provenance attestation
(`gh attestation verify <archive> --repo transcodeworks/roadcase`).
[cargo-binstall](https://github.com/cargo-bins/cargo-binstall) fetches the one for your
machine, and builds from source where there is none:

```sh
cargo binstall roadcase
```

Or build it yourself, with Rust 1.88 or later:

```sh
cargo install roadcase
cargo install --git https://github.com/transcodeworks/roadcase roadcase    # the main branch
```

Until the crates are on crates.io, only the last line works, and cargo-binstall takes
`--git https://github.com/transcodeworks/roadcase`.

On macOS you also need Xcode's command-line tools; on Windows, [NSIS](https://nsis.sourceforge.io);
on Linux, [`appimagetool`](https://github.com/AppImage/appimagetool) on `PATH`, the
AppImage runtime for your architecture (`runtime-x86_64` or `runtime-aarch64`, from the
AppImage project's releases) named by `APPIMAGE_RUNTIME`, and binutils (`objcopy`,
`strip`) when you keep symbols. roadcase passes the runtime to `appimagetool`, so it never
downloads one. A `.deb` needs `dpkg-deb` (on any Debian or Ubuntu), and `dpkg-shlibdeps`
(`dpkg-dev`) to find its library dependencies; a pacman package needs `makepkg`, on Arch
Linux, run as someone other than root (in CI, an Arch Linux container on a Linux runner of
the architecture: a native build, never a cross-build).

### 2. Make an update key

Once per application:

```sh
roadcase keygen keys/
```

`keys/update.pub` is the public half: commit it, and build it into your application.
`keys/update.key` is the secret half: put it in your secret store, never in the
repository. It has no password, and whoever holds it can update every installed copy.
If you lose it, installed copies can no longer be updated, so keep a backup.

If you keep the key in a file instead, on a laptop or a backup drive, add `--password`:
`roadcase keygen --password keys/` encrypts `update.key` with a password you type twice
(or give in `ROADCASE_UPDATE_KEY_PASSWORD`), in minisign's own format, so the `minisign`
tool reads it too. A key kept in a secret store gains nothing from a password stored
beside it.

### 3. Describe your application

A `roadcase.toml` beside your `Cargo.toml`:

```toml
[app]
name = "Hello"
id = "com.example.hello"
version = { file = "Cargo.toml", regex = '(?m)^version = "(.*)"' }

[[app.build]]
run = ["cargo", "build", "--release"]

[macos.executable]
from = "target/release/hello"

[macos.dmg]
[macos.notarize]
[macos.update_archive]

[feed]
url = "https://updates.example.com/hello/{channel}/"
public_key = { file = "keys/update.pub" }
```

Check it:

```sh
roadcase check
```

`check` reports every problem at once, and refuses keys it does not know, so a typo
never goes unnoticed.

### 4. Package

```sh
roadcase app                 # the unsigned .app, to look at
roadcase release macos       # build, sign, notarize, disk image
```

With nothing else set, the release is signed ad hoc and not notarized: enough to try it
on your own Mac. For a release other people can open, the environment says who signs and
how Apple is reached (each has a key in the config, if you prefer other names):

| Variable | What it is |
| --- | --- |
| `CODESIGN_IDENTITY` | Your Developer ID Application identity, by name or SHA-1 hash |
| `CODESIGN_KEYCHAIN` | The only keychain to look in (optional) |
| `CODESIGN_CERTIFICATE`, `CODESIGN_CERTIFICATE_PASSWORD` | Instead of both, in CI: the certificate as a base64 `.p12`, and its password. The release imports it into a temporary keychain and removes that afterwards |
| `NOTARY_PROFILE` | A profile stored with `xcrun notarytool store-credentials` |
| `NOTARY_KEY`, `NOTARY_KEY_ID`, `NOTARY_ISSUER` | Or an App Store Connect API key: the `.p8` file's path or its text |
| `NOTARY_APPLE_ID`, `NOTARY_PASSWORD`, `NOTARY_TEAM_ID` | Or an Apple ID and its app-specific password |

A Developer ID gets the hardened runtime and a secure timestamp. Every program is signed
on its own, inside out, with the entitlements and identifier the config gives it; the
application and then the disk image are notarized and stapled; and the update archive is
made from the stapled application, so an update opens with no network. See
[roadcase-bundle](crates/roadcase-bundle#macos).

On Windows, add a `[windows]` table and your NSIS script, and run `roadcase release
windows` on Windows:

```toml
[windows.executable]
from = "target/release/hello.exe"

[windows.nsis]
script = "packaging/installer.nsi"

# Optional: any command that signs one file.
[windows.sign]
command = ["signtool", "sign", "/fd", "SHA256", "/a", "{file}"]
```

[`examples/helper-app/packaging/windows/installer.nsi`](examples/helper-app/packaging/windows/installer.nsi)
is a script to start from: it installs for one person, registers your URL schemes and
document types, and reads the flags an update starts it with. See
[roadcase-bundle](crates/roadcase-bundle#windows).

On Linux, add a `[linux]` table (an icon and a category are required) and run, on each
architecture's machine:

```toml
[linux]
icons = ["icons/256.png", "icons/icon.svg"]
categories = ["Utility"]

[linux.executable]
from = "target/release/hello"

[linux.appimage]
```

```sh
roadcase app --system linux  # the AppDir, to look at (works on any system)
roadcase release linux       # build, AppDir, AppImage
```

`[linux.deb]` and `[linux.pacman]` make system packages from the same AppDir; `release
linux` makes every format the config has, or those `--format appimage|deb|pacman` names
(a `.deb` on Debian and a pacman package on Arch, say, from one config):

```toml
[linux.deb]
maintainer = "Example <packages@example.com>"

# Optional: the .deb installs your apt repository as a source, as browsers and
# editors do, so apt upgrade finds the next release.
[linux.deb.apt_source]
url = "https://packages.example.com/apt"
key = "keys/repo/key.gpg"

[linux.pacman]
license = ["MIT"]
```

### 5. Publish an update

With a `[feed]` table and the update key's secret half in `ROADCASE_UPDATE_SECRET_KEY`,
`roadcase release` ends by writing its part of the release into the feed folder
(`[feed] out`, `target/feed` by default): the macOS update archive, the Windows installer
under a versioned name, or the AppImage, with `latest.json` and its signature. A `.deb`
and a pacman package are announced at their place in your apt or pacman repository
(`[linux.deb] repository` or `apt_source`, `[linux.pacman] repository`) and never copied
into the feed. Without the key, the release is packaged and nothing is published.

Each system's build machine adds its own part to the same version, so sync the folder
down from your host first when another machine already published; then upload it,
artifacts first and `latest.json` last.

The same by hand, for artifacts made elsewhere:

```sh
roadcase publish --version 1.2.0 --out feed/ --notes "What changed." \
    --artifact macos-aarch64=app-tar-gz=target/roadcase/Hello-1.2.0-aarch64.app.tar.gz
```

`publish` reads the secret key from `ROADCASE_UPDATE_SECRET_KEY` (or `--key`). A key with
a password takes it from `ROADCASE_UPDATE_KEY_PASSWORD` (or the variable that
`--password-env` names), or asks for it in a terminal. Check a feed the way an
installed copy will:

```sh
roadcase verify --key keys/update.pub feed/
```

### 6. Linux packages: repositories and the AUR

A `.deb` and a pacman package are updated by the system's package manager, from a
repository you publish as a folder of static files. roadcase signs it with an OpenPGP key,
in Rust, with no `gpg` or agent. Once:

```sh
roadcase repo keygen keys/repo --name "Example Packages <packages@example.com>"
```

`keys/repo/signing.key` is the secret key (`--password` encrypts it): put it in your secret
store, as `ROADCASE_REPO_SIGNING_KEY`. `key.asc` and `key.gpg` are the public key; they are
also written into each repository.

For each release, sync the repository folder down from your host, add the packages, and
upload it again with deletions (each package keeps its newest three versions; `--keep`):

```sh
roadcase repo apt repo/apt --add target/roadcase/hello_1.2.0-1_amd64.deb --origin Example
roadcase repo pacman repo/arch --name example --add target/roadcase/hello-1.2.0-1-x86_64.pkg.tar.zst
```

People add the apt repository with its key in `/usr/share/keyrings` and a deb822 source
(or install your first `.deb` by hand, when it installs the source itself):

```sh
sudo curl -fsSLo /usr/share/keyrings/hello-archive-keyring.gpg https://packages.example.com/apt/key.gpg
printf 'Types: deb\nURIs: https://packages.example.com/apt\nSuites: stable\nComponents: main\nSigned-By: /usr/share/keyrings/hello-archive-keyring.gpg\n' \
    | sudo tee /etc/apt/sources.list.d/hello.sources
sudo apt update && sudo apt install hello
```

and the pacman repository with its key trusted and a section in `/etc/pacman.conf`:

```sh
curl -fsSL https://packages.example.com/arch/key.asc | sudo pacman-key --add -
sudo pacman-key --lsign-key <fingerprint>
printf '\n[example]\nServer = https://packages.example.com/arch/$arch\n' | sudo tee -a /etc/pacman.conf
sudo pacman -Sy hello
```

On Arch, the AUR is the other route; an application uses either or both. `roadcase repo
aur` writes a `hello-bin` `PKGBUILD` and `.SRCINFO` that repackage the pacman packages as
the repository publishes them (one per architecture, each with its SHA-256):

```sh
roadcase repo aur --url https://packages.example.com/arch --out aur/ \
    --maintainer "Example <packages@example.com>" target/roadcase/hello-1.2.0-1-*.pkg.tar.zst
```

Publishing it is yours or your CI's: commit both files to
`ssh://aur@aur.archlinux.org/hello-bin.git` and push. Upload the repository first, since
the `PKGBUILD` downloads from it.

Announce the release in the feed too, so installed copies hear of it. `roadcase release
linux` does, once the config names the repositories:

```toml
[linux.deb]
repository = "https://packages.example.com/apt"     # or [linux.deb.apt_source] url

[linux.pacman]
repository = "https://packages.example.com/arch"
```

By hand it is `--artifact linux-x86_64-deb=deb=<file>` with `--url
linux-x86_64-deb=<where the repository publishes it>`, and the same for
`linux-x86_64-pacman=pacman=<file>`.

### 7. Update from your application

```toml
[dependencies]
roadcase-update = { git = "https://github.com/transcodeworks/roadcase" }
```

```rust
use roadcase_update::{Config, Updater};

const PUBLIC_KEY: &str = include_str!("../keys/update.pub");

let updater = Updater::new(Config::new(
    "https://updates.example.com/hello/stable/",
    PUBLIC_KEY,
    env!("CARGO_PKG_VERSION"),
    cache_dir.join("updates"),
)?)?;

if let Some(update) = updater.check()? {
    // Ask the person first: nothing is downloaded or installed unasked.
    let staged = updater.download(&update, |received, total| { /* progress */ })?;
    staged.install(&[])?;
    std::process::exit(0); // the installer waits for the application to leave
}
```

A copy installed from a `.deb` or a pacman package finds the release under its own kind,
and `download` answers `Error::ManagedBySystem { manager }`: show "Update with your
package manager" (`update.managed_by()` names apt or pacman before you ask).

On Linux, also call `roadcase_update::desktop::install()` once at startup. An AppImage
has no installer, so this is what puts it in the menu and registers its URL schemes and
document types; it does nothing when nothing changed and answers `Error::NotAppImage`
when the application does not run from an AppImage.

The calls block, so run them on a thread of your own. The library has no UI and no UI
toolkit dependency. See [roadcase-update](crates/roadcase-update) for your own
transport, what installing does on each system, and the file formats.

## Going further

Two complete configs show what larger applications need:

- [`examples/helper-app`](examples/helper-app/roadcase.toml): a helper process with its
  own entitlements, an ffmpeg sidecar, a bundled manual, document types, a URL scheme, a
  test variant that installs beside the real app, a Windows installer signed with Azure
  Artifact Signing, and a Linux AppImage.
- [`examples/extension-app`](examples/extension-app/roadcase.toml): a File Provider
  extension and a command-line tool built with Swift, an application group named after
  the signing team, a launch agent, and Open With entries.

Every key is documented in
[`crates/roadcase-bundle/src/config.rs`](crates/roadcase-bundle/src/config.rs); values
can come from the environment or from files, and strings can hold placeholders such as
`{version}` and `{team}` ([details](crates/roadcase-bundle#placeholders)).

## Crates

| Crate | What it is |
| --- | --- |
| [`roadcase`](crates/roadcase) | The command-line tool. |
| [`roadcase-bundle`](crates/roadcase-bundle) | The packager behind `roadcase check`, `app` and `release`, as a library. |
| [`roadcase-update`](crates/roadcase-update) | The updater your application links, and the feed format. |
| [`roadcase-repo`](crates/roadcase-repo) | Signed apt and pacman repositories and AUR `-bin` packages, behind `roadcase repo`. |

## Contributing

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

No test needs a signing identity, Xcode, Linux's tools, `gpg` or the network: the command
lines of `codesign`, `notarytool`, `hdiutil`, `makensis` and the Windows signing command
are built as values and checked on every system. Three tests use a system's own tools
where they are: one splits a real executable's symbols (Linux only), one makes a whole
ad hoc signed macOS release with `codesign`, `hdiutil` and `tar` (macOS only), and one
compiles the example installer script when `makensis` is on `PATH`.

On a Mac, `scripts/macos-e2e.sh` goes through the whole macOS path: it packages a tiny
application with a helper program (`scripts/e2e-app`) as 1.0.0 and 2.0.0 with `roadcase
release macos`, signed ad hoc, checks the signatures and each program's entitlements, the
dSYMs, the disk image, the update archive and the feed, serves the feed on 127.0.0.1, and
lets 1.0.0 update itself to 2.0.0 by swapping its bundle. With a Developer ID Application
identity in the keychain it also signs a release with it and checks the hardened runtime,
the timestamp and what Gatekeeper says; with `NOTARY_PROFILE` set, it notarizes that
release and checks the stapled tickets.

On Windows, `pwsh scripts/windows-e2e.ps1` does the same for the installer: it packages
the same application unsigned through the example NSIS script, installs 1.0.0 silently,
checks the files and the registry (uninstall entry, URL scheme, document type), lets
1.0.0 update itself to 2.0.0 from a feed on 127.0.0.1 (`/S /UPDATE /RELAUNCH=`), and
uninstalls. It needs NSIS and python, and installs for the current person only.

On Linux, `scripts/linux-e2e.sh` goes through the whole AppImage path: it packages a
tiny application (`scripts/linux-e2e`) as 1.0.0 and 2.0.0 with `roadcase release linux`,
publishes both to a feed served on 127.0.0.1, lets 1.0.0 update itself to 2.0.0 and
start again, and installs and removes its desktop integration under a temporary
`$HOME`. It uses `$APPIMAGETOOL` and `$APPIMAGE_RUNTIME`, or downloads them for the run,
and runs AppImages with `APPIMAGE_EXTRACT_AND_RUN=1` where there is no FUSE. From a Mac
or any system with Docker, in a Linux container of the host's architecture:

```sh
docker run --rm -v "$PWD":/src -v roadcase-target:/target -e CARGO_TARGET_DIR=/target \
    -w /src rust:1.99.1-bookworm sh -c \
    'apt-get update -qq && apt-get install -y -qq desktop-file-utils shared-mime-info >/dev/null &&
     scripts/linux-e2e.sh'
```

`scripts/deb-e2e.sh` does the same for the `.deb`, as root in a throwaway Debian or Ubuntu
container: it packages 1.0.0 and 2.0.0, serves a signed apt repository on 127.0.0.1,
installs from it through a `Signed-By` source, checks that 1.0.0 hears of 2.0.0 from the
feed and leaves it to apt (`ManagedBySystem`), then `apt upgrade`s, trims the repository
and purges the package. `scripts/pacman-e2e.sh` does it on Arch Linux with makepkg,
`pacman -Sy`/`-Syu` from a signed pacman repository (and refuses a database signed by
another key), and builds and installs the AUR `-bin` package with makepkg, checking its
`.SRCINFO` against `makepkg --printsrcinfo`. Each script's header has its `docker run`
line. The Arch Linux image is x86_64 only: on an aarch64 host it runs emulated
(`--platform linux/amd64`, with pacman's sandbox disabled, which qemu cannot run), with
the test's own programs built beforehand for x86_64 (`scripts/pacman-e2e.sh prebuild`).

## Releasing

The four crates share one version, one `CHANGELOG.md`, one tag (`v<version>`) and one
GitHub release. Commits follow [Conventional Commits](https://www.conventionalcommits.org)
(`feat:`, `fix:`, `docs:`, `ci:`, `chore:`; `!` or a `BREAKING CHANGE:` footer for a
breaking change): the next version and the changelog are worked out from them.

[release-plz](https://release-plz.dev) (`release-plz.toml`, `.github/workflows/release.yml`)
keeps a pull request named `chore: release v<version>` open against `main`. Merging it is
the whole release: the workflow tags it, drafts the GitHub release, builds the `roadcase`
program for each target on that target's own system, signs and notarizes the macOS ones
and signs the Windows one, attaches each archive with its SHA-256 and attestation, and
publishes the release once all of them are there. A missing signing secret stops the job
that needs it, and the release stays a draft until that job is run again. A macOS program
cannot carry its notarization ticket (only apps, disk images and packages can be
stapled), so Gatekeeper asks Apple for it the first time the program runs.

The workflow's header lists the repository secrets it reads: a GitHub App's id and private
key (so that CI runs on the release pull request), the Developer ID certificate and
notarization account, and the Azure Artifact Signing account.

### Publishing to crates.io

The crates are `publish = false`, and the workflow's `crates` job runs only when the
repository variable `PUBLISH_ENABLED` is `true`. That job publishes through crates.io
[trusted publishing](https://crates.io/docs/trusted-publishing): a short-lived token from
the workflow's own identity, no stored secret. crates.io lets a crate name its trusted
publisher only once the crate exists, so the first version is published by hand:

1. Set `publish = true` under `[workspace.package]` in `Cargo.toml`, and merge the release
   pull request. `PUBLISH_ENABLED` is not set yet, so the release is tagged and its
   binaries attached, and nothing goes to crates.io.
2. From a checkout of that tag, with a crates.io token made for this one use
   (scopes `publish-new` and `publish-update`): `cargo publish --workspace --locked`. Revoke
   the token.
3. On crates.io, in each of the four crates' settings, add a trusted publisher: GitHub,
   repository `transcodeworks/roadcase`, workflow `release.yml`, environment `crates-io`.
4. Create the `crates-io` environment in the repository's settings and set the repository
   variable `PUBLISH_ENABLED` to `true`. Every later release publishes itself.

## Licence

MIT; see [LICENSE](LICENSE).
