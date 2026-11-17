# roadcase-repo

Signed package repositories as folders of static files, for the `.deb` and pacman
packages roadcase makes, and AUR `-bin` packages. It is the library behind `roadcase repo`.
Upload each folder as it is to any HTTPS host.

Signing is OpenPGP in Rust, with [rPGP](https://github.com/rpgp/rpgp) (MIT or Apache-2.0):
no `gpg`, no `gpg-agent`, no keyring on the build machine. The key is one Ed25519 primary
key (v4, EdDSA) that certifies itself and signs; signatures use SHA-512. apt verifies them
with `gpgv` (Debian 12) and `sqv` (Debian 13), pacman with GnuPG; both are checked end to
end by the repository's scripts.

## Status

| Part | State |
| --- | --- |
| Signing key: generate (optionally with a password), read from `ROADCASE_REPO_SIGNING_KEY`, export armored and binary | done |
| Detached (binary and armored) and cleartext signatures, and their verification | done |
| `.deb` control files read without dpkg (ar, control.tar gz/xz/zstd) | done |
| apt repository: pool, `Packages`/`.gz`, `Release`, `Release.gpg`, `InRelease`, retention | done |
| pacman packages read without pacman (`.pkg.tar.zst`/`.xz`/`.gz`) | done |
| pacman repository: `.db`/`.files` (as `repo-add` writes them), package and database signatures, retention | done |
| AUR `-bin` `PKGBUILD` and `.SRCINFO` | done (`.SRCINFO` checked against `makepkg --printsrcinfo`) |
| Pushing to the AUR | not done: a `git push` that stays the release's own step |

## apt

```text
<dir>/
  key.asc, key.gpg                                     the public key
  pool/<component>/<p>/<package>/<package>_<version>_<arch>.deb
  dists/<suite>/Release, Release.gpg, InRelease
  dists/<suite>/<component>/binary-<arch>/Packages, Packages.gz
```

[`apt::add`](src/apt.rs) copies `.deb` files into the pool, and refuses one whose name is
taken by other contents: a published version never changes. [`apt::index`](src/apt.rs)
reads every `.deb` in the pool, keeps the newest `keep` versions of each package and
architecture (dpkg's version order), deletes the older files, and writes `dists/<suite>`
again. `Release` lists SHA-256 sums only. Packages for `all` are listed under every
architecture.

## pacman

```text
<dir>/
  key.asc                                              the public key
  <arch>/<pkgname>-<pkgver>-<pkgrel>-<arch>.pkg.tar.zst, .sig
  <arch>/<repo>.db, <repo>.db.tar.gz, <repo>.files, <repo>.files.tar.gz, each with .sig
```

`Server = https://<host>/<path>/$arch` reads it. [`pacman::index`](src/pacman.rs) reads
every package in each architecture's folder, keeps the newest `keep` versions of each
(pacman's `vercmp` order), signs those without a valid `.sig`, and lists the newest of each
in the databases (pacman holds one version per name; older files stay for links that still
name them). The `.db` and `.files` are copies of their `.tar.gz`, not links, since static
hosts serve none.

## AUR

[`aur::generate`](src/aur.rs) reads one release's packages (one per architecture) and
writes a `<pkgname>-bin` `PKGBUILD` whose `source_<arch>` is
`<url>/<arch>/<file>` with its `sha256sums_<arch>`, which `provides` and `conflicts` with
the original name and unpacks the package's files with `bsdtar`. Commit `PKGBUILD` and
`.SRCINFO` to `ssh://aur@aur.archlinux.org/<pkgname>-bin.git` and push, after the
repository is uploaded.

## Tests

Unit tests build `.deb`s and pacman packages in memory, make repositories of several
versions and architectures, and verify every signature with the same library. No test runs
`gpg`, dpkg, pacman or the network; `scripts/deb-e2e.sh` and `scripts/pacman-e2e.sh` do,
in containers.
