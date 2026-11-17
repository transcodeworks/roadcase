# roadcase-bundle

Packages a desktop application from one `roadcase.toml`. It is the library behind the
`roadcase` command; the schema, with every key documented, is in
[`src/config.rs`](src/config.rs).

What it is for: applications whose bundles are more than one executable. Every piece of
nested code gets its own entitlements and signing identifier, app extensions land in
`Contents/PlugIns`, launch agents in `Contents/Library/LaunchAgents`, both the app and the
disk image are notarized, symbols are kept as dSYMs and stripped from what ships, the
Windows installer is the application's own NSIS script, the Linux AppImage carries a
desktop entry, icons and MIME types for the application to register, and the `.deb` and
pacman package install the same files the conventional way.

## Status

| Part | State |
| --- | --- |
| Parsing, validation (every problem at once, unknown keys refused) | done |
| Values from the environment, files (with a regex) and defaults; placeholders | done |
| Build commands (`[[app.build]]`, `[[macos.extension.build]]`), feed URL and key passed to the build | done |
| macOS `.app` assembly: layout, Info.plist, programs, resources, icons, extensions, launch agents, PkgInfo | done |
| Variants (test identifier: other id, Alternate rank, imported types, no URL scheme) | done |
| Signing plan (inside out, per-step entitlements and identifier), entitlement rendering | done |
| `codesign` inside out, ad hoc or with an identity; hardened runtime and timestamp for a Developer ID | done; ad hoc and Developer ID checked on macOS 27 (aarch64) by `scripts/macos-e2e.sh` |
| dSYMs (`dsymutil`), strip, UUID check, symbols zip | done; checked by `scripts/macos-e2e.sh` |
| Disk image (`hdiutil`, Applications link), signed; `.app.tar.gz` update archive | done; checked by `scripts/macos-e2e.sh`, with a bundle-swap update from it |
| Signing certificate from a base64 `.p12` in a temporary keychain | done; checked by hand with a self-signed certificate |
| Notarization and stapling of the app and the disk image (`notarytool`: profile, API key or Apple ID) | done, the commands ported from release scripts that ship notarized applications; **not yet run from roadcase**: `scripts/macos-e2e.sh` does it with `NOTARY_PROFILE` |
| Windows stage, NSIS defines, generated include (files, registry, update flags), `makensis` | done; generation tested on every system, and the example script compiled with `makensis` on macOS |
| Windows signing command (executables, uninstaller, installer) | done; command lines tested, **never run against a signing service from roadcase** |
| Windows install, silent update and uninstall of what the installer makes | `scripts/windows-e2e.ps1`, **on Windows in CI only** |
| Linux AppDir: programs, resources, hicolor icons, `.desktop` entry, shared-mime-info, AppStream metainfo, AppRun | done |
| Linux `.debug` files (`objcopy`), strip, debug link, symbols zip | done (runs on Linux) |
| AppImage from `appimagetool` with a local runtime | done (runs on Linux) |
| `.deb` from `dpkg-deb`: `/opt/<package>` tree, control file, `dpkg-shlibdeps`, optional apt source as a conffile | done (runs on Linux; checked on Debian 12 and 13) |
| pacman package from `makepkg` and a generated `PKGBUILD` | done (runs on Arch Linux; checked on x86_64) |
| Publishing from the config: the update archive, the installer, the AppImage; `.deb` and pacman package by repository URL | done; macOS checked by `scripts/macos-e2e.sh`, the rest by tests |

## Use

```rust,no_run
use roadcase_bundle::{Options, Project, macos};

let project = Project::load("roadcase.toml".as_ref(), Options::macos().with_variant(None))?;
roadcase_bundle::commands::build(&project, true)?;
let app = macos::assemble(&project, "target/roadcase".as_ref())?;
for step in &app.signing {
    println!("{} {:?}", step.path.display(), step.entitlements);
}
# Ok::<(), roadcase_bundle::Error>(())
```

## macOS

`roadcase release macos`, or `macos::release` after `macos::Signing::prepare`, runs
on a Mac, with the system's tools and Xcode's command-line tools:

1. **Who signs.** `[macos.signing] identity` (`$CODESIGN_IDENTITY`; `-`, ad hoc, by
   default), looked up in `keychain` (`$CODESIGN_KEYCHAIN`) when set. Its certificate says
   whether it is a Developer ID and which team it belongs to, which fills `{team}` unless
   `team` is set. With `certificate` (`$CODESIGN_CERTIFICATE`, a base64 `.p12`, and
   `certificate_password`), the certificate is first imported into a temporary keychain,
   which is removed when the release ends, however it ends; with the default identity,
   the certificate's own identity then signs. This happens before the build, so build
   commands may use `{team}`.
2. **Symbols.** Each program with `symbols = true` gets a dSYM from `dsymutil` in
   `<out>/<name>.dSYMs/`, before it is stripped; the release stops if a dSYM's UUID is not
   its stripped program's. `[macos.symbols]` zips them.
3. **Signing.** `codesign --force`, once per piece, inside out: programs, extensions,
   then the bundle, each with its own entitlements (placeholders filled, written to
   `<out>/<name>.entitlements/`) and identifier. `--deep` is only used to verify. A
   Developer ID adds `--options runtime` and `--timestamp`; `hardened_runtime` and
   `timestamp` (`auto`, `always`, `never`) override that.
4. **Notarization.** With `[macos.notarize]` and an identity that is not ad hoc, the
   application is zipped, submitted with `notarytool submit --wait`, stapled, and assessed
   by `spctl`; when Apple refuses, the error carries its log. Credentials are the first
   complete set of: `profile` (`$NOTARY_PROFILE`); `key`, `key_id`, `issuer`
   (`$NOTARY_KEY`, a `.p8` path or its text, `$NOTARY_KEY_ID`, `$NOTARY_ISSUER`);
   `apple_id`, `password`, `team_id` (`$NOTARY_APPLE_ID`, `$NOTARY_PASSWORD`,
   `$NOTARY_TEAM_ID`). An ad hoc release skips this step and says so.
5. **Disk image.** `[macos.dmg]`: the stapled application and a link to `/Applications`
   (`applications_link = false` leaves it out), made by `hdiutil`, signed, and notarized
   and stapled too unless `staple` leaves the `dmg` out.
6. **Update archive.** `[macos.update_archive]`: `tar` with `COPYFILE_DISABLE=1`, from the
   stapled application, so an update opens with no network and its seal holds.

Passwords and keys never appear in an error: a failed command is shown with them masked.

For a tool other than roadcase that needs the certificate, `roadcase keychain import`
makes the same temporary keychain and prints its path, and `roadcase keychain remove
<path>` removes it.

## Windows

`roadcase release windows`, or `windows::installer`, stages what is installed in
`<out>/<name>.windows/`, laid out as the install folder (the executable, the programs
beside it, each resource under its `to`), signs the executables, and runs `makensis` (from
`PATH`, or `$MAKENSIS`) with the application's own script.

The script receives the config as defines (`-DNAME=...`): `NAME`, `ID`, `VERSION`,
`VERSION4`, `PUBLISHER`, `OUT`, `STAGE`, `EXE`, `EXE_NAME`, `PROGRAM_<NAME>`,
`RESOURCE_<TO>`, `ICON`, `UNINSTALL_KEY`, `OPEN_COMMAND`, `URL_COMMAND`, `URL_SCHEMES`,
`SILENT_FLAG`, `UPDATE_FLAG`, `RELAUNCH_FLAG`, `ALSO_ACCEPT_<n>`, `SIGN_UNINSTALLER` and
your own `defines`; [`config::Nsis`](src/config.rs) has the table. It must write the
installer to `${OUT}`.

It also receives `ROADCASE_NSH`, a generated include whose macros save writing lists by
hand:

| Macro | What it does |
| --- | --- |
| `ROADCASE_INSTALL_FILES` | Every staged file into `$INSTDIR`. A program that is running is moved aside as `<name>.<n>.old` and removed by the next install |
| `ROADCASE_UNINSTALL_FILES` | Removes those files, and their folders once empty |
| `ROADCASE_REGISTER`, `ROADCASE_UNREGISTER` | Under `HKCU\Software\Classes`: each URL scheme as a URL protocol, and each document type's ProgID. `rank = "owner"` or `"default"` makes the application the default for the extension, keeping the earlier default for the uninstaller to put back; other ranks only add it to Open With |
| `ROADCASE_WAIT_UNTIL_WRITABLE` | Waits up to ten seconds for the application that started an update to leave |
| `ROADCASE_READ_UPDATE` | `$R0` is 1 when the installer runs as an update (`/UPDATE`, or a flag of `update_flags.also_accept`), `$R1` the arguments of `/RELAUNCH=` |

[`examples/helper-app/packaging/windows/installer.nsi`](../../examples/helper-app/packaging/windows/installer.nsi)
uses all of them, and is the script `scripts/windows-e2e.ps1` installs, updates and
uninstalls with.

`[windows.sign] command` runs once per file, `{file}` being the file: on the main
executable and each program before packaging, on the uninstaller while `makensis` writes
it (the script's `!uninstfinalize '${SIGN_UNINSTALLER}'`), and on the installer after.
It runs on Windows only, and only when the variable `when_env` names is set, so a
machine without credentials still builds an unsigned installer. On macOS or Linux
`makensis` makes the same installer from a cross-built executable, unsigned, to try it
on a Windows machine.

## Publishing

With `[feed]`, a release ends with `publish::publish`: the secret key comes from
`[feed] secret_key`, or `$ROADCASE_UPDATE_SECRET_KEY` (a password, when the key has one,
from `$ROADCASE_UPDATE_KEY_PASSWORD`); without a key, nothing is published. The feed
folder (`out`, `target/feed` by default, placeholders such as `{channel}` filled)
receives the artifact, `latest.json` and its signature, and keeps the other systems'
parts of the same version.

| Made | Published as | Where |
| --- | --- | --- |
| `.app.tar.gz` | `macos-<arch>`, `app-tar-gz` | copied into the feed folder |
| NSIS installer | `windows-x86_64`, `nsis` | copied under `[windows.nsis] feed_name` |
| AppImage | `linux-<arch>`, `appimage` | copied into the feed folder |
| `.deb` | `linux-<arch>-deb`, `deb` | its URL in the apt repository: `[linux.deb] repository` (or `apt_source.url`) + `/pool/<component>/<p>/<package>/<package>_<version>_<arch>.deb` |
| pacman package | `linux-<arch>-pacman`, `pacman` | its URL in the pacman repository: `[linux.pacman] repository` + `/<arch>/<file>` |

A package without its repository in the config is not announced. The disk image is
never in the feed: people download it from your site.

## Linux

`[linux]` describes an AppDir, assembled in Rust (so `roadcase app --system linux` shows
it on any system), and `[linux.appimage]` the AppImage made from it with `appimagetool`
on Linux. The layout:

```text
<name>.AppDir/
  AppRun                              starts usr/bin/<executable>
  <id>.desktop, <id>.png|.svg, .DirIcon
  usr/bin/<executable>, usr/bin/<program>...
  usr/share/applications/<id>.desktop
  usr/share/icons/hicolor/<size>/apps/<id>.png, scalable/apps/<id>.svg
  usr/share/mime/packages/<id>.xml
  usr/share/metainfo/<id>.metainfo.xml
```

- The desktop entry's `MimeType` lists `x-scheme-handler/<scheme>` for each URL scheme
  and each `[[document_type]]` (its `mime_type`, or `application/x-<extension>`).
  `Exec` ends in `%u` with URL schemes, `%F` with documents only, or
  `exec_arguments`.
- Document types with `rank = "owner"` are declared in `usr/share/mime/packages/<id>.xml`
  with their extensions as globs. A variant with `types = "imported"` declares none.
- Programs with `symbols = true` keep their debug information in
  `<name>.debug/<file>.debug` (`objcopy --only-keep-debug`), are stripped and get a
  `.gnu_debuglink`; the `.debug` files are zipped as `[linux.symbols] zip` says.
- `appimagetool` comes from `PATH` (or `$APPIMAGETOOL`), and the runtime from
  `$APPIMAGE_RUNTIME` (or `linux.appimage.runtime`): without one, packaging stops rather
  than let `appimagetool` download it.
- Nothing is installed by the AppImage: the application calls
  `roadcase_update::desktop::install()` at startup.
- `.rpm` and Flatpak are not made; they would be tables beside `[linux.appimage]`.

### `.deb` and pacman packages

`[linux.deb]` and `[linux.pacman]` package the same AppDir (after its symbols are split)
as the system installs it:

```text
/opt/<package>/bin/<executable>, bin/<program>...   the AppDir's usr/, moved under /opt/<package>
/opt/<package>/lib/..., share/...                    (resources and the $ORIGIN/../lib rpath keep working)
/opt/<package>/.roadcase-install                     deb or pacman: how roadcase-update knows
/usr/bin/<executable>                                a link to /opt/<package>/bin/<executable>
/usr/share/applications/<id>.desktop                 the AppImage's entry, without X-AppImage-Version
/usr/share/icons/hicolor/..., /usr/share/mime/packages/<id>.xml, /usr/share/metainfo/<id>.metainfo.xml
```

- No maintainer scripts and no install hooks: dpkg's triggers and pacman's hooks
  (desktop-file-utils, shared-mime-info, the icon cache) refresh the desktop's caches.
- Versions: `1.2.0-beta.1` is `1.2.0~beta.1-1` for Debian and `1.2.0beta.1-1` for
  pacman, so a pre-release sorts before its release.
- The `.deb` is made by `dpkg-deb --root-owner-group`. Its `Depends` are those declared,
  then what `dpkg-shlibdeps` finds the ELF files need, when it is on `PATH` (`shlibdeps =
  false` turns it off). `[linux.deb.apt_source]` installs a deb822
  `/etc/apt/sources.list.d/<package>.sources` (a conffile) and the repository's key in
  `/usr/share/keyrings/<package>-archive-keyring.gpg` (or `.asc`), named in `Signed-By`.
- The pacman package is made by `makepkg --nodeps` from a `PKGBUILD` that copies the tree
  (`options=(!strip !debug)`, since symbols are already split), so `.PKGINFO`, `.MTREE`
  and `.BUILDINFO` are makepkg's own. It runs on Arch Linux as someone other than root;
  nothing is downloaded. `depends` are declared only.
- `roadcase release linux --format deb` (or `pacman`, `appimage`) makes only those.

## Placeholders

Strings in the config may hold `{name}`, `{id}`, `{version}`, `{build_number}`, `{arch}`
(`aarch64`), `{apple_arch}` (`arm64`), `{triple}` (`aarch64-apple-darwin`,
`x86_64-unknown-linux-gnu`), `{team}`, `{channel}`, `{env:NAME}` and
the names of `[vars]`. `{{` and `}}` are literal braces. Entitlements and plists given as
files have the placeholders in their strings filled too, so an application group can be
written `{team}.com.example.app` once, in `[vars]`.

## Values

Where a key takes a value (the version, the signing identity, the feed key), it is a
string, or a table looked up in order:

```toml
version = { env = "APP_VERSION", file = "Cargo.toml", regex = '(?m)^version = "(.*)"', default = "0.0.0" }
```

## Tests

`tests/examples.rs` assembles the repository's two example configs from fixture files in a
temporary folder and checks their signing commands, `tests/linux.rs` the helper app's
AppDir, `tests/windows.rs` its Windows stage, defines, include and signing command, and
`tests/publish.rs` the feed each system writes. Every external command is built as a
`process::Invocation` before it runs, so its command line is tested on any system, with
no signing identity, Xcode, Linux tool or network. Three tests run a system's own tools
where they are: one splits a real executable's symbols with `objcopy` (Linux only),
`tests/macos_release.rs` makes a whole ad hoc signed release (macOS only), and one
compiles the example installer script when `makensis` is on `PATH`.
