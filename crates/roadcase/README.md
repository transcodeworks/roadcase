# roadcase

The command-line tool.

```sh
cargo install --path crates/roadcase
```

## Updates

```sh
roadcase keygen keys/                    # update.key (secret) and update.pub, once per application
roadcase keygen --password keys/         # the same, with update.key encrypted, for a key kept in a file
roadcase publish --version 1.2.0 --out feed/ --notes "What changed." \
    --artifact windows-x86_64=nsis=dist/App-1.2.0-x64-setup.exe \
    --artifact linux-x86_64=appimage=target/roadcase/App-1.2.0-x86_64.AppImage
roadcase verify --key keys/update.pub feed/
```

`publish` reads the secret key from `--key`, or `ROADCASE_UPDATE_SECRET_KEY`, or else the
older name `CC_UPDATE_SECRET_KEY`. An artifact published somewhere else than the feed
folder, as a `.deb` is in its apt repository, takes `--url <target>=<url>`: it is then
measured and named by that URL, and not copied. `roadcase release` does all of this from
the config; `publish` is for artifacts made another way.

`keygen --password` asks for the password twice without echoing it, or reads it from
`ROADCASE_UPDATE_KEY_PASSWORD` when that is set. Without `--password`, `update.key` has no
password, for a secret store, as before. `publish` reads either kind of key; for an
encrypted one it takes the password from the variable `--password-env NAME` names, or
`ROADCASE_UPDATE_KEY_PASSWORD`, or else asks in a terminal, and fails when there is no
terminal to ask in. A wrong password fails on the key's checksum before anything is
written. Keys made by `minisign -G` (which encrypts by default) work the same way.

`keygen`, `publish` and `verify` were the `cc-update` tool's subcommands, with the same
arguments and output, and the password options added; see [roadcase-update](../roadcase-update/README.md).

## Packaging

```sh
roadcase check [--config roadcase.toml] [--variant test] [--system macos|windows|linux]
roadcase app [--config roadcase.toml] [--variant test] [--system macos|linux] [--out target/roadcase] [--build]
roadcase release macos|windows|linux [--config roadcase.toml] [--variant test] [--out target/roadcase] [--skip-build] [--format appimage|deb|pacman]...
```

- `check` reads and validates the config and prints what it resolves to.
- `app` assembles the unsigned macOS `.app` and prints its path; on standard error it lists
  what will be signed, in order, with which entitlements and identifier. With
  `--system linux` it assembles the Linux AppDir instead, on any system.
- `release` runs the whole release for one system, on that system, and prints what it
  made.
  - `release macos` builds, assembles the `.app`, makes dSYMs and strips, signs inside
    out (ad hoc unless `CODESIGN_IDENTITY` names an identity, or `CODESIGN_CERTIFICATE`
    holds a base64 `.p12`), notarizes and staples the application, makes, signs, notarizes
    and staples the disk image, and makes the update archive. An ad hoc release is not
    notarized.
  - `release windows` builds, stages the install folder, signs the executables with
    `[windows.sign]`, runs `makensis` with your script, and signs the installer. Off
    Windows it makes the installer unsigned.
  - `release linux` builds, assembles the AppDir, splits and strips symbols, and makes the
    AppImage with `appimagetool`, the `.deb` with `dpkg-deb` and the pacman package with
    `makepkg`, as the config has them, or only the formats `--format` names.

  With a `[feed]` and the update key in `ROADCASE_UPDATE_SECRET_KEY` (its password, if it
  has one, in `ROADCASE_UPDATE_KEY_PASSWORD`), each ends by writing its part of the
  release into the feed folder, with `latest.json` and its signature; a `.deb` and a
  pacman package are announced at their repository's URL. Without the key it says that
  nothing was published. See [roadcase-bundle](../roadcase-bundle/README.md) for what
  each step reads from the config and the environment.

## A signing certificate in CI

```sh
roadcase keychain import [--certificate-env CODESIGN_CERTIFICATE] \
    [--password-env CODESIGN_CERTIFICATE_PASSWORD] [--keychain <file>]
roadcase keychain remove <file>
```

`release macos` needs neither: with `CODESIGN_CERTIFICATE` set (a base64 `.p12`:
`base64 -i certificate.p12`), it imports the certificate into a temporary keychain itself
and removes the keychain when it ends. `keychain import` is for other tools that sign: it
makes the keychain (in `$RUNNER_TEMP`, or the system's temporary folder) with a random
password, imports the certificate so that `codesign` uses it without asking, puts the
keychain first on the search list, and prints its path, for `CODESIGN_KEYCHAIN` or
`codesign --keychain`. `keychain remove` deletes it and takes it off the search list.

## Linux package repositories

```sh
roadcase repo keygen keys/repo --name "Example Packages <packages@example.com>" [--password]
roadcase repo apt repo/apt [--add app_1.2.0-1_amd64.deb]... [--suite stable] [--codename stable] \
    [--component main] [--origin Example] [--label Example] [--description ...] [--keep 3]
roadcase repo pacman repo/arch --name example [--add app-1.2.0-1-x86_64.pkg.tar.zst]... [--keep 3]
roadcase repo aur --url https://packages.example.com/arch --out aur/ [--maintainer ...] [--pkgname app-bin] \
    app-1.2.0-1-x86_64.pkg.tar.zst app-1.2.0-1-aarch64.pkg.tar.zst
```

- `repo keygen` writes `signing.key` (the armored OpenPGP secret key, for a secret store),
  `key.asc` and `key.gpg` (the public key, armored and binary), and prints the
  fingerprint. It refuses to replace a key: installed copies trust the old one.
- `repo apt` and `repo pacman` sign with the key in `ROADCASE_REPO_SIGNING_KEY`, or the
  file `--key` names; a key with a password takes it from
  `ROADCASE_REPO_SIGNING_KEY_PASSWORD` (or the variable `--password-env` names). They add
  the packages, keep the newest `--keep` versions of each (removing older files), and write
  the indexes again from the packages in the folder, so sync the folder down first and up
  again with deletions. See [roadcase-repo](../roadcase-repo/README.md) for the layouts.
- `repo aur` writes `PKGBUILD` and `.SRCINFO` for the AUR; pushing them is yours.
