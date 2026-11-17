# roadcase-update

Signed self-updates for desktop applications. It has no UI and depends on no UI toolkit, so
a gpui, Slint or headless application uses it the same way.

A release is a folder of static files on any HTTPS host:

| File | What it is |
| --- | --- |
| `latest.json` | The manifest: the version, and for each platform an artifact with its size and SHA-256 |
| `latest.json.minisig` | A [minisign](https://jedisct1.github.io/minisign/) signature of the manifest |
| `App-1.2.0-x64-setup.exe` | Windows: an NSIS installer |
| `App-1.2.0-aarch64.app.tar.gz` | macOS: the signed, notarized `.app` |
| `App-1.2.0-x86_64.AppImage` | Linux: the AppImage |
| `app_1.2.0-1_amd64.deb`, `app-1.2.0-1-x86_64.pkg.tar.zst` | Linux packages: announced only, installed by apt and pacman |

The application holds the public key. The one signature covers the version and every
artifact's hash, so a host that is taken over can neither change an artifact nor offer an
older release. Artifacts come over HTTPS only, and a download that differs from the manifest
is removed.

## In the application

```toml
roadcase-update = { git = "<this repository>", rev = "<commit-sha>" }
```

```rust
use roadcase_update::{Config, Updater};

let updater = Updater::new(Config::new(
    "https://updates.example.com/app/stable/", // the folder that holds latest.json
    PUBLIC_KEY,                                 // update.pub, compiled in
    env!("CARGO_PKG_VERSION"),
    cache_dir.join("updates"),                  // a folder of the application's own
)?)?;
if let Some(update) = updater.check()? {
    let staged = updater.download(&update, |received, total| { /* progress */ })?;
    staged.install(&[])?;      // or &["--background"]: the arguments to start again with
    std::process::exit(0);     // the installer waits for this process: leave now
}
```

Every call blocks, so call them from a thread of the application's own. A download that
was finished by an earlier run is found again and not fetched twice.

The default `http` feature fetches with reqwest and rustls. Without it, the application
gives `Updater::with_transport` its own `Transport`.

### What installing does

- **Windows.** The installer is started with `/S /UPDATE`, and `/RELAUNCH=<arguments>`
  when there are any. `/S` is NSIS's silent mode; the application's script reads the other
  two, and starts the application again when the files are in place:

  ```nsis
  !include "FileFunc.nsh"

  Section -Relaunch
    ${GetParameters} $0
    ClearErrors
    ${GetOptions} $0 "/UPDATE" $1
    ${IfNot} ${Errors}
      ${GetOptions} $0 "/RELAUNCH=" $1
      Exec '"$INSTDIR\app.exe" $1'
    ${EndIf}
  SectionEnd
  ```

  The application leaves as soon as `install` returns, and may still be leaving when the
  installer starts, so the script waits until the executable can be written before it
  replaces it. `examples/helper-app/packaging/windows/installer.nsi` in this repository is
  a whole script that does, with the macros `roadcase release windows` generates. Install for one person (`RequestExecutionLevel user`), so that an update
  needs no administrator.
- **macOS.** The archive is unpacked beside the running bundle. `codesign --verify --deep
  --strict` must pass, and the bundle must be signed by `Config::macos_team`, or by the team
  that signed the running one when that is `None`. A shell script then waits for the
  application to leave, swaps the two bundles, and opens the new one; the old bundle comes
  back if the new one cannot take its place. An application that runs from a folder the
  person cannot change, or straight from a disk image, answers `Error::NotWritable`.

  Make the archive with `COPYFILE_DISABLE=1 tar -czf App.app.tar.gz -C <folder> App.app`:
  without it, macOS adds `._` files that break the bundle's seal.
- **Linux.** The application must run from its AppImage: the file `$APPIMAGE` names, which
  the AppImage runtime sets. The download is copied into a hidden file beside it,
  checked again against the manifest's SHA-256, checked to be an AppImage and made
  executable. Where it goes then depends on the running AppImage's name:

  - A name that holds the running version, as a token of its own
    (`App-1.2.0-x86_64.AppImage`, not `App-11.2.0-x86_64.AppImage`): the new release
    takes the name its artifact has in the feed, the last segment of its URL
    (`App-1.3.0-x86_64.AppImage`), beside the old one, over any other file of that name.
    Once it is in place, the old file is removed; if that fails, it stays. An artifact
    name with a path separator, or not ending in `.AppImage`, is not used, and the
    update replaces the file in place instead.
  - Any other name (`App-x86_64.AppImage`, `App.AppImage`): it is renamed over the
    running file, which keeps its name.

  Both are one rename, which the running copy survives. A shell script then waits for
  the application to leave and starts the new file with the arguments.
  An application not started from an AppImage answers `Error::NotAppImage`; one whose
  AppImage is in a folder the person cannot change, `Error::NotWritable`.

  An AppImage has no installer, so the application registers itself with the desktop,
  at every start:

  ```rust
  match roadcase_update::desktop::install() {
      Ok(_changed) | Err(roadcase_update::Error::NotAppImage) => {}
      Err(error) => eprintln!("desktop integration: {error}"),
  }
  ```

  It copies the desktop entry the packager made (its `Exec` and `TryExec` now the
  AppImage's path), the shared-mime-info file of the document types it owns and its
  hicolor icons from the AppImage into `$XDG_DATA_HOME` (`~/.local/share`), and runs
  `update-desktop-database` and `update-mime-database` when they are there and something
  changed. Then links of its URL schemes and its documents open it. Calling it again
  changes nothing unless the AppImage moved or a release changed the files.
  Installed menu entries follow the file: a release that took a new name starts with
  that name as its `$APPIMAGE`, and its first `install()` points the entry there.
  Shortcuts a person made by hand to the old path are not updated, and stop working
  when the old file is removed; a stable file name avoids that.
  `roadcase_update::desktop::remove()` takes it all away, for a "remove from the menu"
  action. It does not make the application the default for types others also open; the
  person chooses that.

- **Linux packages.** A copy installed from a `.deb` or a pacman package is apt's or
  pacman's to update, from the application's repository (or the AUR). The packages
  roadcase makes hold a marker, `/opt/<package>/.roadcase-install`, saying `deb` or
  `pacman`, beside the folder that holds the executable. `roadcase_update::package_manager()`
  reads it (through `/proc/self/exe`, so the `/usr/bin` link leads there; never inside an
  AppImage), and `Config::new` keeps it in `Config::package`. Such a copy looks for its
  release under `<target>-deb` or `<target>-pacman` (`linux-x86_64-deb`), never under the
  AppImage's key; `check()` reports it as usual, `Update::managed_by()` names the package
  manager, and `download()` answers `Error::ManagedBySystem { manager }` without fetching
  anything:

  ```rust
  if let Some(update) = updater.check()? {
      match update.managed_by() {
          Some(manager) => show(format!("{} is out: update with {manager}", update.version)),
          None => { /* download and install */ }
      }
  }
  ```

  A copy whose package is not in the feed hears of nothing, so announce the package only
  once the repository holds it.

Arguments to start again with may only hold letters, digits and `-_=.:`, since they pass
through an installer's command line.

## Making releases

Releases are made with the `roadcase` command-line tool, in this repository's
`crates/roadcase`:

```sh
cargo install --git <this repository> --rev <commit-sha> roadcase
```

Once, for each application:

```sh
roadcase keygen keys/
```

`keys/update.pub` is compiled into the application. `keys/update.key` goes into a secret
store: it has no password, and whoever holds it can update every installed copy. A new key
locks out every copy that holds the old one, so `keygen` refuses to replace a key.

For each release, on each platform's build machine:

```sh
roadcase publish --version 1.2.0 --out feed/ --notes "What changed." \
    --artifact windows-x86_64=nsis=dist/App-1.2.0-x64-setup.exe
```

Kinds are `nsis`, `app-tar-gz` and `appimage` (for `linux-x86_64` and `linux-aarch64`),
and `deb` and `pacman`, published under `linux-x86_64-deb`, `linux-aarch64-pacman` and so
on (`publish` refuses them under any other key, and any other kind under those). A
package lives in its repository, so give its place there with `--url
linux-x86_64-deb=https://...` (`Part::url`): the manifest then names that URL, and the
package is not copied into the feed. `roadcase release` publishes each system's part
itself, from the application's `roadcase.toml`.
The key comes from `--key keys/update.key` or from `ROADCASE_UPDATE_SECRET_KEY`. The name
it had before this crate was renamed, `CC_UPDATE_SECRET_KEY`, is still read when
`ROADCASE_UPDATE_SECRET_KEY` is not set. `publish` copies
the artifact into `feed/` and writes `latest.json` and its signature there. A `latest.json`
of the same version keeps its other platforms, so sync `feed/` from the bucket first when
another machine already added its part. Then upload the folder as it is, artifacts before
`latest.json` and its signature.

```sh
roadcase verify --key keys/update.pub feed/
```

checks a feed folder the way an application will.

## Formats

The formats are fixed, since installed copies hold public keys in them:

- `update.pub` is one base64 line of minisign's public key: `Ed`, an 8-byte key id and
  the 32-byte Ed25519 public key.
- `update.key` is minisign's secret key file: an untrusted comment, then one base64 line
  holding `Ed`, the KDF algorithm, the `B2` checksum algorithm, the KDF's 32-byte salt
  and its 8-byte little-endian opslimit and memlimit, the key id, the 64-byte Ed25519
  secret key and a BLAKE2b-256 checksum of `Ed`, the key id and the secret key. Without a
  password (`generate`, `roadcase keygen`) the KDF is two zero bytes and its salt and
  limits are zero. With one (`generate_with_password`, `roadcase keygen --password`) it is
  `Sc`: scrypt over the password, with the stored salt and with N, r and p derived from the
  opslimit and memlimit as libsodium does, gives 104 bytes that are XORed over the key id,
  secret key and checksum, so a wrong password shows as a checksum that does not match.
  This is minisign's own format: keys from `minisign -G` read with their password, and
  `minisign` and `rsign` read ours. `sign_with_password` and `publish_with_password` take
  either kind; `sign` and `publish` take a key without a password.
- `latest.json.minisig` is a minisign signature of `latest.json`'s exact bytes.
- `latest.json` is the [`Manifest`](src/manifest.rs), written by `publish` as pretty JSON.

This crate was called `cc-update`; keys, signatures and feeds made under that name work
unchanged, and `tests/compat.rs` checks fixtures made by it.

## What it does not do

- **Delta updates.** Every update is the whole installer or bundle.
- **Install Linux packages.** Only AppImages update themselves; a `.deb` or pacman package
  is announced and left to apt or pacman, and an `.rpm` or Flatpak is not known. A manifest
  may carry kinds this version does not know, and ignores them, as versions before
  `appimage` ignore it and versions before `deb` and `pacman` ignore those (under keys they
  never look up).
- **Elevation.** An application installed for every user of a computer cannot update itself.
- **Rollback of a version.** A release that is published stays the newest until a higher
  version replaces it.
