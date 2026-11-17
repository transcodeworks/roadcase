//! The `roadcase.toml` schema.
//!
//! One file per application. Paths are relative to the folder that holds the
//! file. Every string that names a value (a path, a plist string, an argument)
//! may hold placeholders, which [`crate::Context`] fills:
//!
//! | Placeholder | Value |
//! | --- | --- |
//! | `{name}` | [`App::name`], or the variant's |
//! | `{id}` | [`App::id`], or the variant's |
//! | `{version}` | the resolved [`App::version`] |
//! | `{build_number}` | the resolved [`App::build_number`], the version by default |
//! | `{arch}` | `aarch64` or `x86_64` |
//! | `{apple_arch}` | `arm64` or `x86_64`, as Apple's tools name it |
//! | `{triple}` | the Rust target triple of this machine, such as `aarch64-apple-darwin` or `x86_64-unknown-linux-gnu` |
//! | `{team}` | the Apple team, from [`MacosSigning::team`] or, once signing exists, the certificate |
//! | `{channel}` | the resolved [`Feed::channel`] |
//! | `{env:NAME}` | the environment variable `NAME`, which must be set |
//! | `{<var>}` | a value from `[vars]` |
//!
//! `{{` and `}}` stand for literal braces.
//!
//! Unknown keys are errors, so a misspelt key never passes silently.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The whole file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// What the application is.
    pub app: App,
    /// Named values for placeholders, such as an application group built from
    /// the team: `app_group = "{team}.example"`.
    #[serde(default)]
    pub vars: BTreeMap<String, Value>,
    /// Document types the application opens, on every system.
    #[serde(default, rename = "document_type")]
    pub document_types: Vec<DocumentType>,
    /// The macOS bundle.
    pub macos: Option<Macos>,
    /// The Windows installer.
    pub windows: Option<Windows>,
    /// The Linux application: an AppDir, and the AppImage made from it.
    pub linux: Option<Linux>,
    /// Where releases are published for the application's updater.
    pub feed: Option<Feed>,
    /// Named variants, chosen with `--variant`, such as a test identifier that
    /// installs beside the real application.
    #[serde(default, rename = "variant")]
    pub variants: BTreeMap<String, Variant>,
}

/// A value that may come from somewhere other than the file itself.
///
/// Written either as a string (placeholders filled), or as a table that looks
/// in order at an environment variable, a file, and a default:
///
/// ```toml
/// version = { env = "APP_VERSION", file = "Cargo.toml", regex = '(?m)^version = "(.*)"' }
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Value {
    /// A string, placeholders filled.
    Text(String),
    /// A lookup.
    Lookup(Lookup),
}

/// Where a [`Value`] is looked up. The first that answers wins.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lookup {
    /// An environment variable; set and not empty, it wins.
    pub env: Option<String>,
    /// A file: its whole text, trimmed, or what `regex` captures in it.
    pub file: Option<PathBuf>,
    /// A regular expression whose first group is the value in `file`.
    pub regex: Option<String>,
    /// The value when nothing else answers. Placeholders are filled.
    pub default: Option<String>,
}

/// What the application is, on every system.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct App {
    /// The name people see: `Name.app`, the installer's title.
    pub name: String,
    /// The reverse-DNS identifier, such as `com.example.app`.
    pub id: String,
    /// The version, in semantic versioning: what the updater compares.
    pub version: Value,
    /// `CFBundleVersion` on macOS. The version when absent.
    pub build_number: Option<Value>,
    /// Who publishes it: the Windows uninstall entry, the installer's details.
    pub publisher: Option<String>,
    /// The copyright line.
    pub copyright: Option<String>,
    /// URL schemes the application opens, such as `example` for `example:`
    /// links. Registered on every system (on Linux as
    /// `x-scheme-handler/<scheme>` in the desktop entry).
    #[serde(default)]
    pub url_schemes: Vec<String>,
    /// Commands run, in order, before anything is packaged: typically the
    /// cargo build.
    #[serde(default)]
    pub build: Vec<Command>,
}

/// A command run by the packager.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    /// The program and its arguments, as a list: no shell, no splitting.
    pub run: Vec<String>,
    /// Where it runs. The config's folder by default.
    pub cwd: Option<PathBuf>,
    /// Extra environment for it.
    #[serde(default)]
    pub env: BTreeMap<String, Value>,
}

/// One kind of document, for the macOS bundle, the Windows registry and the
/// Linux desktop entry and shared-mime-info file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentType {
    /// Its name: `CFBundleTypeName`, and the Windows ProgID's description.
    pub name: String,
    /// File extensions, without the dot.
    pub extensions: Vec<String>,
    /// Uniform type identifiers it is (macOS `LSItemContentTypes`).
    #[serde(default)]
    pub content_types: Vec<String>,
    /// What the application does with it.
    #[serde(default)]
    pub role: Role,
    /// How strongly the application claims it. On Windows, `owner` and
    /// `default` make the application the default handler; `alternate` and
    /// `none` only add it to Open With.
    #[serde(default)]
    pub rank: Rank,
    /// The Windows ProgID. `{id}.<name without spaces>` when absent.
    pub prog_id: Option<String>,
    /// An icon file in the bundle's Resources, for macOS.
    pub icon: Option<String>,
    /// Its MIME type on Linux, such as `application/x-example-project`.
    /// `application/x-<first extension>` when absent. When the application
    /// owns the type (`rank = "owner"`), the AppImage declares it in a
    /// shared-mime-info file, with the extensions as globs.
    pub mime_type: Option<String>,
}

/// `CFBundleTypeRole`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    /// Reads and writes it.
    #[default]
    Editor,
    /// Only reads it.
    Viewer,
    /// Provides runtime services for it.
    Shell,
    /// Declares it and does nothing with it.
    None,
}

/// `LSHandlerRank`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Rank {
    /// The application made this type.
    #[default]
    Owner,
    /// The default handler, without owning the type.
    Default,
    /// One handler among others.
    Alternate,
    /// Never chosen to open it.
    None,
}

/// A program file: one path, or candidates of which the first that exists
/// and is not empty wins (an empty file counts as missing).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Source {
    /// One path.
    One(String),
    /// Candidates, in order.
    Candidates(Vec<String>),
}

/// The application's own executable.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Executable {
    /// Where the build put it.
    pub from: Source,
    /// Its name in the bundle or install folder. On macOS the application's
    /// name, on Windows `<name>.exe`, on Linux the name in lower case with
    /// `-` for spaces (`Example Studio` is `example-studio`), when absent.
    pub name: Option<String>,
    /// Keeps its symbols out of what ships. macOS: dSYM, then strip, then
    /// check that both have the same UUID. Linux: a `.debug` file
    /// (`objcopy --only-keep-debug`), then strip, then a debug link to it.
    #[serde(default)]
    pub symbols: bool,
}

/// The macOS bundle.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Macos {
    /// The main executable: `Contents/MacOS/<name>`.
    pub executable: Executable,
    /// The bundle's entitlements, which the main executable runs with.
    pub entitlements: Option<PathBuf>,
    /// An Info.plist to start from. Keys this file describes override it.
    pub info_plist: Option<PathBuf>,
    /// `LSMinimumSystemVersion`, such as `11.0`.
    pub min_system: Option<String>,
    /// `LSApplicationCategoryType`, such as `public.app-category.productivity`.
    pub category: Option<String>,
    /// An `.icns` file, copied as `Resources/<name>.icns`.
    pub icon: Option<PathBuf>,
    /// An `Assets.car` (icon styles of macOS 26), copied as `Resources/Assets.car`.
    pub assets_car: Option<PathBuf>,
    /// `CFBundleIconName`: the app icon's name in `assets_car`.
    pub icon_name: Option<String>,
    /// `NSHighResolutionCapable`.
    #[serde(default = "yes")]
    pub high_resolution: bool,
    /// Uniform types the application declares as its own.
    #[serde(default, rename = "exported_type")]
    pub exported_types: Vec<TypeDeclaration>,
    /// Uniform types the application uses and others declare.
    #[serde(default, rename = "imported_type")]
    pub imported_types: Vec<TypeDeclaration>,
    /// Any other Info.plist keys, such as usage descriptions. Strings have
    /// their placeholders filled.
    #[serde(default)]
    pub plist: toml::Table,
    /// Other executables, beside the main one in `Contents/MacOS`.
    #[serde(default, rename = "program")]
    pub programs: Vec<Program>,
    /// Files and folders copied into the bundle.
    #[serde(default, rename = "resource")]
    pub resources: Vec<Resource>,
    /// App extensions, in `Contents/PlugIns`.
    #[serde(default, rename = "extension")]
    pub extensions: Vec<Extension>,
    /// Launch agents, in `Contents/Library/LaunchAgents`, which the
    /// application registers with `SMAppService`.
    #[serde(default, rename = "launch_agent")]
    pub launch_agents: Vec<LaunchAgent>,
    /// How code is signed.
    #[serde(default)]
    pub signing: MacosSigning,
    /// Notarization, for a Developer ID release.
    pub notarize: Option<Notarize>,
    /// The disk image people install from.
    pub dmg: Option<Dmg>,
    /// The archive the updater installs from.
    pub update_archive: Option<UpdateArchive>,
    /// Where the dSYMs of programs with `symbols = true` are collected.
    pub symbols: Option<Symbols>,
}

fn yes() -> bool {
    true
}

/// A uniform type declaration (`UTExportedTypeDeclarations` or
/// `UTImportedTypeDeclarations`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeDeclaration {
    /// `UTTypeIdentifier`.
    pub identifier: String,
    /// `UTTypeDescription`.
    pub description: Option<String>,
    /// `UTTypeConformsTo`, such as `public.data`.
    #[serde(default)]
    pub conforms_to: Vec<String>,
    /// File extensions, without the dot.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// MIME types.
    #[serde(default)]
    pub mime_types: Vec<String>,
    /// `UTTypeIconFile`.
    pub icon: Option<String>,
}

/// An executable beside the main one, signed on its own.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Program {
    /// Its file name in the bundle: what the application looks for.
    pub name: String,
    /// Where it comes from.
    pub from: Source,
    /// Its own entitlements. Without them it is signed with none.
    pub entitlements: Option<PathBuf>,
    /// The signing identifier, when it must not be the file name (a tool that
    /// runs as the application, say: the application's id).
    pub identifier: Option<String>,
    /// dSYM, strip and UUID check, as for the main executable.
    #[serde(default)]
    pub symbols: bool,
    /// Whether the packager signs it. A program that arrives signed by its
    /// maker may say `false`.
    #[serde(default = "yes")]
    pub sign: bool,
}

/// Files copied into the bundle.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resource {
    /// A file, a folder, or a glob such as `docs/*.md`. A glob that matches
    /// nothing is an error.
    pub from: String,
    /// The folder they go into, relative to `Contents`, such as
    /// `Resources/docs`. A folder keeps its name inside it.
    pub to: String,
}

/// An app extension (`.appex`), such as a File Provider.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Extension {
    /// Its bundle name: `Contents/PlugIns/<name>.appex`.
    pub name: String,
    /// Commands that build it, run before packaging, after `[app] build`.
    #[serde(default)]
    pub build: Vec<Command>,
    /// A finished `.appex` to copy as it is. Or else `executable` and
    /// `info_plist`, from which the packager makes one.
    pub bundle: Option<PathBuf>,
    /// The extension's executable: `Contents/MacOS/<file name>`.
    pub executable: Option<Source>,
    /// The extension's Info.plist, with its `NSExtension` dictionary. The
    /// packager sets its identifier, executable and the application's version.
    pub info_plist: Option<PathBuf>,
    /// `CFBundleIdentifier`: `{id}.<name>` when absent.
    pub id: Option<String>,
    /// Extra Info.plist keys, strings with placeholders filled.
    #[serde(default)]
    pub plist: toml::Table,
    /// Its own entitlements.
    pub entitlements: Option<PathBuf>,
}

/// A launch agent plist.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchAgent {
    /// Its `Label`; the file is `<label>.plist`.
    pub label: String,
    /// A plist to copy, strings with placeholders filled. Or else the fields
    /// below make one.
    pub from: Option<PathBuf>,
    /// `BundleProgram`, relative to the bundle, such as `Contents/MacOS/app`.
    pub program: Option<String>,
    /// `ProgramArguments`.
    #[serde(default)]
    pub arguments: Vec<String>,
    /// `RunAtLoad`.
    #[serde(default)]
    pub run_at_load: bool,
    /// `KeepAlive`.
    #[serde(default)]
    pub keep_alive: bool,
    /// Extra keys.
    #[serde(default)]
    pub plist: toml::Table,
}

/// How macOS code is signed. Code is signed inside out, each piece with its
/// own entitlements; `--deep` is used only to verify.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosSigning {
    /// The identity, by name or SHA-1 hash; `-` signs ad hoc.
    #[serde(default = "default_identity")]
    pub identity: Value,
    /// The only keychain the identity is looked up in.
    #[serde(default = "default_keychain")]
    pub keychain: Option<Value>,
    /// The Apple team, for `{team}`. Read from the identity's certificate
    /// when absent or empty.
    pub team: Option<Value>,
    /// The hardened runtime: `auto` turns it on for a Developer ID.
    #[serde(default)]
    pub hardened_runtime: Toggle,
    /// A secure timestamp: `auto` for a Developer ID.
    #[serde(default)]
    pub timestamp: Toggle,
    /// A signing certificate and its private key, as a base64 `.p12`, for a
    /// machine whose keychain does not hold them (CI). By default
    /// `$CODESIGN_CERTIFICATE`. When it has a value, a release imports it
    /// into a temporary keychain, signs from that keychain, and removes it
    /// afterwards; with the default identity (`-`), the certificate's own
    /// identity signs.
    #[serde(default = "default_certificate")]
    pub certificate: Option<Value>,
    /// The `.p12`'s password. By default `$CODESIGN_CERTIFICATE_PASSWORD`.
    #[serde(default = "default_certificate_password")]
    pub certificate_password: Option<Value>,
}

impl Default for MacosSigning {
    fn default() -> Self {
        Self {
            identity: default_identity(),
            keychain: default_keychain(),
            team: None,
            hardened_runtime: Toggle::Auto,
            timestamp: Toggle::Auto,
            certificate: default_certificate(),
            certificate_password: default_certificate_password(),
        }
    }
}

fn default_certificate() -> Option<Value> {
    env_or_empty("CODESIGN_CERTIFICATE")
}

fn default_certificate_password() -> Option<Value> {
    env_or_empty("CODESIGN_CERTIFICATE_PASSWORD")
}

fn default_identity() -> Value {
    Value::Lookup(Lookup {
        env: Some("CODESIGN_IDENTITY".into()),
        default: Some("-".into()),
        ..Lookup::default()
    })
}

fn default_keychain() -> Option<Value> {
    Some(Value::Lookup(Lookup {
        env: Some("CODESIGN_KEYCHAIN".into()),
        default: Some(String::new()),
        ..Lookup::default()
    }))
}

/// On, off, or decided by the identity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Toggle {
    /// On for a Developer ID identity, off otherwise.
    #[default]
    Auto,
    /// Always on.
    Always,
    /// Always off.
    Never,
}

/// Notarization credentials, the first of these that is complete: a stored
/// `notarytool` profile; an App Store Connect API key (`key`, `key_id`,
/// `issuer`); or an Apple ID, its app-specific password and the team. An ad
/// hoc signature is never notarized: a release signed with `-` skips this.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Notarize {
    /// A profile stored with `xcrun notarytool store-credentials`.
    #[serde(default = "default_profile")]
    pub profile: Option<Value>,
    /// The Apple ID.
    #[serde(default = "default_apple_id")]
    pub apple_id: Option<Value>,
    /// Its app-specific password.
    #[serde(default = "default_password")]
    pub password: Option<Value>,
    /// Its team.
    #[serde(default = "default_team_id")]
    pub team_id: Option<Value>,
    /// An App Store Connect API key: the `.p8` file's path, or its text
    /// (`-----BEGIN PRIVATE KEY-----`...), as a secret store holds it. By
    /// default `$NOTARY_KEY`.
    #[serde(default = "default_notary_key")]
    pub key: Option<Value>,
    /// The API key's id. By default `$NOTARY_KEY_ID`.
    #[serde(default = "default_notary_key_id")]
    pub key_id: Option<Value>,
    /// The API key's issuer, a UUID. By default `$NOTARY_ISSUER`.
    #[serde(default = "default_notary_issuer")]
    pub issuer: Option<Value>,
    /// What is notarized and stapled. The application always comes first,
    /// since an update never passes through the disk image.
    #[serde(default = "default_notarized")]
    pub staple: Vec<Notarized>,
}

fn env_or_empty(name: &str) -> Option<Value> {
    Some(Value::Lookup(Lookup {
        env: Some(name.into()),
        default: Some(String::new()),
        ..Lookup::default()
    }))
}

fn default_profile() -> Option<Value> {
    env_or_empty("NOTARY_PROFILE")
}
fn default_apple_id() -> Option<Value> {
    env_or_empty("NOTARY_APPLE_ID")
}
fn default_password() -> Option<Value> {
    env_or_empty("NOTARY_PASSWORD")
}
fn default_team_id() -> Option<Value> {
    env_or_empty("NOTARY_TEAM_ID")
}
fn default_notary_key() -> Option<Value> {
    env_or_empty("NOTARY_KEY")
}
fn default_notary_key_id() -> Option<Value> {
    env_or_empty("NOTARY_KEY_ID")
}
fn default_notary_issuer() -> Option<Value> {
    env_or_empty("NOTARY_ISSUER")
}
fn default_notarized() -> Vec<Notarized> {
    vec![Notarized::App, Notarized::Dmg]
}

/// Something notarized and stapled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Notarized {
    /// The `.app`.
    App,
    /// The disk image.
    Dmg,
}

/// The disk image: the application, and a link to `/Applications` beside it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dmg {
    /// Its file name.
    #[serde(default = "default_dmg_name")]
    pub name: String,
    /// The volume's name. The application's name when absent.
    pub volume_name: Option<String>,
    /// Whether an `Applications` link sits beside the application.
    #[serde(default = "yes")]
    pub applications_link: bool,
}

fn default_dmg_name() -> String {
    "{name}-{version}-{arch}.dmg".into()
}

/// The `.app.tar.gz` the updater installs, made without `._` files.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateArchive {
    /// Its file name.
    #[serde(default = "default_archive_name")]
    pub name: String,
}

fn default_archive_name() -> String {
    "{name}-{version}-{arch}.app.tar.gz".into()
}

/// The dSYMs, kept out of the bundle.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Symbols {
    /// A zip of all of them, for a crash reporter.
    #[serde(default = "default_symbols_name")]
    pub zip: String,
}

fn default_symbols_name() -> String {
    "{name}-{version}-{arch}.dSYMs.zip".into()
}

/// The Windows installer.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Windows {
    /// The main executable, `<name>.exe` in the install folder by default.
    pub executable: Executable,
    /// Other executables installed beside it.
    #[serde(default, rename = "program")]
    pub programs: Vec<WindowsProgram>,
    /// Files and folders installed, `to` relative to the install folder.
    #[serde(default, rename = "resource")]
    pub resources: Vec<Resource>,
    /// The `.ico` for the installer and the uninstall entry.
    pub icon: Option<PathBuf>,
    /// The NSIS installer.
    pub nsis: Nsis,
    /// What the installer writes to the registry.
    #[serde(default)]
    pub registry: Registry,
    /// How executables are signed. Unsigned when absent.
    pub sign: Option<WindowsSign>,
}

/// An executable installed beside the main one.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsProgram {
    /// Its file name in the install folder, with `.exe`.
    pub name: String,
    /// Where it comes from.
    pub from: Source,
    /// Whether the packager signs it.
    #[serde(default = "yes")]
    pub sign: bool,
}

/// The NSIS script and what it is given.
///
/// The script is the application's own, so that an installer can carry logic
/// no generic template has (taking over an earlier install, asking a running
/// copy to leave). The packager stages what is installed in a folder laid out
/// as the install folder, signs the executables in it, and runs `makensis`
/// from `PATH` (or `$MAKENSIS`) with these defines:
///
/// | Define | Value |
/// | --- | --- |
/// | `NAME`, `ID`, `VERSION`, `VERSION4` | the application; `VERSION4` is four numbers, for `VIProductVersion` |
/// | `PUBLISHER`, `COPYRIGHT` | `app.publisher` (the name when absent), `app.copyright` when set |
/// | `OUT` | the installer to write |
/// | `STAGE` | the staged folder |
/// | `EXE`, `EXE_NAME` | the staged main executable, and its file name |
/// | `PROGRAM_<NAME>` | each staged program: `example-host.exe` is `PROGRAM_EXAMPLE_HOST` |
/// | `RESOURCE_<TO>` | each staged resource folder: `to = "resources/docs"` is `RESOURCE_RESOURCES_DOCS` (`RESOURCE_ROOT` for the install folder itself) |
/// | `ICON` | the `.ico`, when set |
/// | `INSTALL_MODE` | `current-user` |
/// | `UNINSTALL_KEY` | the registry key under `HKCU`, whole |
/// | `OPEN_COMMAND`, `URL_COMMAND` | the commands, `{exe}` being `$INSTDIR\<EXE_NAME>` |
/// | `URL_SCHEMES` | the URL schemes, separated by spaces, when there are any |
/// | `SILENT_FLAG`, `UPDATE_FLAG`, `RELAUNCH_FLAG` | `/S`, `/UPDATE`, `/RELAUNCH=` |
/// | `ALSO_ACCEPT_<n>` | each of `update_flags.also_accept`, from 1 |
/// | `SIGN_UNINSTALLER` | the signing command with `%1` for the file, for `!uninstfinalize`, when signing |
/// | `ROADCASE_NSH` | a generated include; see below |
/// | yours | `defines` |
///
/// The generated include, `!include "${ROADCASE_NSH}"`, holds macros a script
/// may use instead of writing the same lists by hand:
/// `ROADCASE_INSTALL_FILES` (every staged file into `$INSTDIR`; an executable
/// that is running is moved aside as `<name>.<n>.old`),
/// `ROADCASE_UNINSTALL_FILES`, `ROADCASE_REGISTER` and `ROADCASE_UNREGISTER`
/// (URL schemes and document types under `HKCU\Software\Classes`),
/// `ROADCASE_WAIT_UNTIL_WRITABLE` (an update starts while the application is
/// still leaving) and `ROADCASE_READ_UPDATE` (`$R0` is 1 for an update, `$R1`
/// the arguments to start again with).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Nsis {
    /// The `.nsi` script.
    pub script: PathBuf,
    /// The installer's file name.
    #[serde(default = "default_installer_name")]
    pub output: String,
    /// Its file name in the update feed: versioned, so that a manifest that
    /// still names an older installer finds it.
    #[serde(default = "default_feed_installer_name")]
    pub feed_name: String,
    /// Who it installs for. Only `current-user` exists: an update must need
    /// no administrator.
    #[serde(default)]
    pub install_mode: InstallMode,
    /// The flags an update starts the installer with.
    #[serde(default)]
    pub update_flags: UpdateFlags,
    /// More `/D` defines.
    #[serde(default)]
    pub defines: BTreeMap<String, Value>,
}

fn default_installer_name() -> String {
    "{name}-Setup.exe".into()
}

fn default_feed_installer_name() -> String {
    "{name}-{version}-x64-setup.exe".into()
}

/// Who an installer installs for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallMode {
    /// The person running it, under `%LOCALAPPDATA%`.
    #[default]
    CurrentUser,
}

/// The silent-update convention between the updater and the installer
/// script. The first three must be the updater's own; they are here so the
/// packager can pass them to the script, and refuse a script built for others.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateFlags {
    /// NSIS's silent flag.
    #[serde(default = "default_silent")]
    pub silent: String,
    /// The flag that says an update runs the installer.
    #[serde(default = "default_update")]
    pub update: String,
    /// The flag whose value is the arguments to start again with.
    #[serde(default = "default_relaunch")]
    pub relaunch: String,
    /// Flags an earlier updater used, which the script also honours, such as
    /// `/R` to restart.
    #[serde(default)]
    pub also_accept: Vec<String>,
}

impl Default for UpdateFlags {
    fn default() -> Self {
        Self {
            silent: default_silent(),
            update: default_update(),
            relaunch: default_relaunch(),
            also_accept: Vec::new(),
        }
    }
}

fn default_silent() -> String {
    roadcase_update::NSIS_SILENT.into()
}
fn default_update() -> String {
    roadcase_update::NSIS_UPDATE.into()
}
fn default_relaunch() -> String {
    roadcase_update::NSIS_RELAUNCH.into()
}

/// Registry entries, all under `HKCU`. URL schemes come from
/// [`App::url_schemes`] and file associations from [`Config::document_types`].
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    /// The key under `Software\Microsoft\Windows\CurrentVersion\Uninstall`.
    /// The application's name when absent.
    pub uninstall_key: Option<String>,
    /// The command a URL or a document opens with; `{exe}` is the installed
    /// executable and `%1` the URL or file.
    #[serde(default = "default_open_command")]
    pub open_command: String,
    /// The command a URL opens with, when not `open_command`.
    pub url_command: Option<String>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            uninstall_key: None,
            open_command: default_open_command(),
            url_command: None,
        }
    }
}

fn default_open_command() -> String {
    "\"{exe}\" \"%1\"".into()
}

/// A signing command, run once per file, such as Azure Artifact Signing's
/// CLI. It runs on Windows only: a cross-built installer stays unsigned.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsSign {
    /// The program and its arguments. `{file}` is the file to sign.
    pub command: Vec<String>,
    /// Signing happens only when this environment variable is set, so that a
    /// machine without credentials still builds.
    pub when_env: Option<String>,
    /// Environment for the command, such as the names its tool expects.
    #[serde(default)]
    pub env: BTreeMap<String, Value>,
    /// What is signed.
    #[serde(default = "default_signed")]
    pub targets: Vec<Signed>,
}

fn default_signed() -> Vec<Signed> {
    vec![
        Signed::Executable,
        Signed::Programs,
        Signed::Uninstaller,
        Signed::Installer,
    ]
}

/// Something the Windows signing command signs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Signed {
    /// The main executable.
    Executable,
    /// Every program with `sign = true`.
    Programs,
    /// The uninstaller, while `makensis` writes it.
    Uninstaller,
    /// The installer.
    Installer,
}

/// The Linux application, as an AppDir (assembled in Rust, so it can be
/// looked at on any system) and the AppImage `appimagetool` makes from it on
/// Linux.
///
/// ```text
/// <name>.AppDir/
///   AppRun                      starts usr/bin/<executable>
///   <id>.desktop, <id>.png|svg  what appimagetool reads; .DirIcon
///   usr/bin/<executable>, usr/bin/<program>...
///   usr/share/applications/<id>.desktop
///   usr/share/icons/hicolor/<size>/apps/<id>.png, scalable/apps/<id>.svg
///   usr/share/mime/packages/<id>.xml   the owned document types
///   usr/share/metainfo/<id>.metainfo.xml
/// ```
///
/// An AppImage has no installer: the application registers its desktop
/// entry, URL schemes and document types itself, with
/// `roadcase_update::desktop`, from the files above.
///
/// `[linux.deb]` and `[linux.pacman]` make system packages from the same
/// AppDir, installed the conventional way and updated by the system's
/// package manager:
///
/// ```text
/// /opt/<package>/bin/<executable>, bin/<program>...   the AppDir's usr/, under /opt/<package>
/// /opt/<package>/lib/..., share/...                    (resources keep their places)
/// /opt/<package>/.roadcase-install                     deb or pacman: how it was installed
/// /usr/bin/<executable>                                a link to /opt/<package>/bin/<executable>
/// /usr/share/applications/<id>.desktop
/// /usr/share/icons/hicolor/<size>/apps/<id>.png, scalable/apps/<id>.svg
/// /usr/share/mime/packages/<id>.xml
/// /usr/share/metainfo/<id>.metainfo.xml
/// ```
///
/// The desktop's caches are refreshed by the system's own triggers and
/// hooks (dpkg triggers, pacman hooks), so the packages carry no scripts.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Linux {
    /// The main executable: `usr/bin/<name>`.
    pub executable: Executable,
    /// Other executables, beside the main one in `usr/bin`.
    #[serde(default, rename = "program")]
    pub programs: Vec<LinuxProgram>,
    /// Files and folders copied into the AppDir, `to` relative to its root,
    /// such as `usr/share/example/docs`. Shared libraries go in `usr/lib`,
    /// found through an rpath of `$ORIGIN/../lib`: AppRun sets no library
    /// path, so programs the application starts are not affected.
    #[serde(default, rename = "resource")]
    pub resources: Vec<Resource>,
    /// The application's icons: square PNGs, placed by their size in the
    /// hicolor theme, and an SVG, placed as `scalable`. At least one.
    #[serde(default)]
    pub icons: Vec<PathBuf>,
    /// The desktop entry's `Categories`, such as `AudioVideo`. At least one.
    #[serde(default)]
    pub categories: Vec<String>,
    /// The desktop entry's `Comment`: a line about what the application does.
    pub comment: Option<String>,
    /// What follows the executable in the desktop entry's `Exec`, with at
    /// most one of the field codes `%f`, `%F`, `%u` and `%U`. When absent,
    /// `%u` with URL schemes (a document then arrives as a path or a
    /// `file:` URL), `%F` with document types only, nothing otherwise.
    pub exec_arguments: Option<Vec<String>>,
    /// More desktop entry keys, such as `StartupWMClass` or `Keywords`:
    /// strings (placeholders filled), booleans, or lists of strings.
    #[serde(default)]
    pub desktop: toml::Table,
    /// An AppStream metainfo file, copied with its placeholders filled as
    /// `usr/share/metainfo/<id>.metainfo.xml`.
    pub metainfo: Option<PathBuf>,
    /// The AppImage people install and the updater installs.
    pub appimage: Option<AppImage>,
    /// A Debian package, made with `dpkg-deb`.
    pub deb: Option<Deb>,
    /// An Arch Linux package, made with `makepkg` on Arch Linux.
    pub pacman: Option<Pacman>,
    /// Where the `.debug` files of programs with `symbols = true` are
    /// collected.
    pub symbols: Option<LinuxSymbols>,
}

/// An executable beside the main one on Linux.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinuxProgram {
    /// Its file name in `usr/bin`: what the application looks for.
    pub name: String,
    /// Where it comes from.
    pub from: Source,
    /// A `.debug` file, strip and a debug link, as for the main executable.
    #[serde(default)]
    pub symbols: bool,
}

/// The AppImage, made by `appimagetool` from `PATH` (or `tool`). Nothing is
/// downloaded: the AppImage runtime comes from `runtime`, a file on this
/// machine, and is passed to `appimagetool --runtime-file`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppImage {
    /// Its file name. Versioned, so that a manifest that still names an
    /// older release finds it in the feed.
    #[serde(default = "default_appimage_name")]
    pub name: String,
    /// The AppImage runtime for this architecture (`runtime-x86_64`,
    /// `runtime-aarch64`). By default `$APPIMAGE_RUNTIME`.
    #[serde(default = "default_appimage_runtime")]
    pub runtime: Value,
    /// The `appimagetool` to run.
    #[serde(default = "default_appimage_tool")]
    pub tool: Value,
}

fn default_appimage_name() -> String {
    "{name}-{version}-{arch}.AppImage".into()
}

fn default_appimage_runtime() -> Value {
    Value::Lookup(Lookup {
        env: Some("APPIMAGE_RUNTIME".into()),
        ..Lookup::default()
    })
}

fn default_appimage_tool() -> Value {
    Value::Lookup(Lookup {
        env: Some("APPIMAGETOOL".into()),
        default: Some("appimagetool".into()),
        ..Lookup::default()
    })
}

/// The Debian package, made by `dpkg-deb` from `PATH` on any Linux. apt
/// updates it, from the application's apt repository (`roadcase repo apt`);
/// the application only learns that a newer version exists.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deb {
    /// The package's name: lower-case letters, digits and `+-.`. The
    /// executable's name when absent. It is also the install folder,
    /// `/opt/<package>`.
    pub package: Option<String>,
    /// Its file name. `<package>_<version>_<arch>.deb` when absent, the
    /// Debian convention (`example_1.2.0-1_amd64.deb`).
    pub name: Option<String>,
    /// The Debian revision, after the version: `1.2.0-1`. A pre-release
    /// sorts before its release (`1.2.0-beta.1` is `1.2.0~beta.1-1`).
    #[serde(default = "default_revision")]
    pub revision: String,
    /// `Maintainer`, such as `Example <packages@example.com>`. `app.publisher`
    /// when absent; one of them is needed.
    pub maintainer: Option<String>,
    /// `Section`.
    #[serde(default = "default_section")]
    pub section: String,
    /// `Priority`.
    #[serde(default = "default_priority")]
    pub priority: String,
    /// `Homepage`.
    pub homepage: Option<String>,
    /// The long description, below the summary line (`linux.comment`, or
    /// the name). Blank lines become ` .`.
    pub description: Option<String>,
    /// `Depends`, written as Debian does: `libgtk-3-0 (>= 3.24)`. Those
    /// `dpkg-shlibdeps` finds are added to them.
    #[serde(default)]
    pub depends: Vec<String>,
    /// `Recommends`.
    #[serde(default)]
    pub recommends: Vec<String>,
    /// The apt repository's URL (the folder that holds `dists/` and
    /// `pool/`), for the update feed: a release with a `[feed]` announces the
    /// `.deb` there, at the place `roadcase repo apt` keeps it
    /// (`<repository>/pool/<component>/<p>/<package>/<package>_<version>_<arch>.deb`),
    /// and never copies the package into the feed. `apt_source.url` when
    /// absent; with neither, the `.deb` is not announced.
    pub repository: Option<String>,
    /// Runs `dpkg-shlibdeps` (from `dpkg-dev`), when it is on `PATH`, over
    /// the package's executables, and adds the libraries' packages to
    /// `depends`. Without it on `PATH`, only `depends` is declared.
    #[serde(default = "yes")]
    pub shlibdeps: bool,
    /// The apt repository the package installs as a source, so that
    /// `apt upgrade` finds the next release: a deb822 file in
    /// `/etc/apt/sources.list.d/<package>.sources` (a conffile) whose
    /// `Signed-By` is the repository's key, installed in
    /// `/usr/share/keyrings/`. Absent, the package installs no source, and
    /// people add the repository themselves.
    pub apt_source: Option<AptSource>,
}

fn default_revision() -> String {
    "1".into()
}

fn default_section() -> String {
    "misc".into()
}

fn default_priority() -> String {
    "optional".into()
}

/// The apt source a `.deb` installs.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AptSource {
    /// The repository's URL: the folder that holds `dists/` and `pool/`.
    pub url: String,
    /// The suite, as `roadcase repo apt --suite` made it.
    #[serde(default = "default_suite")]
    pub suite: String,
    /// The components.
    #[serde(default = "default_components")]
    pub components: Vec<String>,
    /// The repository's public key: `key.gpg` or `key.asc`, as
    /// `roadcase repo keygen` wrote it. Installed as
    /// `/usr/share/keyrings/<package>-archive-keyring.gpg` (or `.asc`).
    pub key: PathBuf,
}

fn default_suite() -> String {
    "stable".into()
}

fn default_components() -> Vec<String> {
    vec!["main".into()]
}

/// The Arch Linux package, made by `makepkg` from a `PKGBUILD` roadcase
/// writes, on Arch Linux (in CI, an Arch Linux container on a Linux runner of
/// the architecture). pacman updates it, from the application's pacman
/// repository (`roadcase repo pacman`) or the AUR (`roadcase repo aur`); the
/// application only learns that a newer version exists.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pacman {
    /// `pkgname`: lower-case letters, digits and `@._+-`. The executable's
    /// name when absent. It is also the install folder, `/opt/<pkgname>`.
    pub pkgname: Option<String>,
    /// `pkgrel`, after the version. A pre-release sorts before its release
    /// (`1.2.0-beta.1` is `1.2.0beta.1`).
    #[serde(default = "default_revision")]
    pub pkgrel: String,
    /// `pkgdesc`: `linux.comment`, or the name, when absent.
    pub description: Option<String>,
    /// `url`.
    pub url: Option<String>,
    /// `license`, as SPDX identifiers such as `MIT`.
    #[serde(default)]
    pub license: Vec<String>,
    /// `depends`, such as `gtk3`. makepkg finds none itself.
    #[serde(default)]
    pub depends: Vec<String>,
    /// `optdepends`, such as `xdg-utils: open links`.
    #[serde(default)]
    pub optdepends: Vec<String>,
    /// The pacman repository's URL (the folder that holds `<arch>/`), for
    /// the update feed: a release with a `[feed]` announces the package
    /// there, at the place `roadcase repo pacman` keeps it
    /// (`<repository>/<arch>/<file>`), and never copies the package into the
    /// feed. Absent, the package is not announced.
    pub repository: Option<String>,
    /// The `makepkg` to run. By default `$MAKEPKG`, or `makepkg`.
    #[serde(default = "default_makepkg")]
    pub makepkg: Value,
}

fn default_makepkg() -> Value {
    Value::Lookup(Lookup {
        env: Some("MAKEPKG".into()),
        default: Some("makepkg".into()),
        ..Lookup::default()
    })
}

/// The `.debug` files, kept out of the AppImage.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinuxSymbols {
    /// A zip of all of them, for a crash reporter.
    #[serde(default = "default_debug_name")]
    pub zip: String,
}

impl Default for LinuxSymbols {
    fn default() -> Self {
        Self {
            zip: default_debug_name(),
        }
    }
}

fn default_debug_name() -> String {
    "{name}-{version}-{arch}.debug.zip".into()
}

/// The update feed: a folder of static files the application polls.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Feed {
    /// The folder URL that holds `latest.json`; `{channel}` names the channel.
    pub url: Value,
    /// The channel this build follows and publishes to.
    #[serde(default = "default_channel")]
    pub channel: Value,
    /// The public key (`update.pub`, or its base64 line), compiled into the
    /// application.
    pub public_key: Value,
    /// The secret key (`update.key`'s text). By default
    /// `ROADCASE_UPDATE_SECRET_KEY`, then `CC_UPDATE_SECRET_KEY`. Without it,
    /// a release packages and publishes nothing. A key with a password takes
    /// it from `ROADCASE_UPDATE_KEY_PASSWORD`.
    pub secret_key: Option<Value>,
    /// The local folder a release is written into, ready to upload as it
    /// is: the artifacts, `latest.json` and its signature. Sync it from the
    /// host first when another machine already added its part of the version.
    #[serde(default = "default_feed_out")]
    pub out: PathBuf,
    /// What changed, for the release notes.
    pub notes: Option<Value>,
    /// The environment variables the build receives the URL and the public
    /// key under, so the application compiles them in.
    #[serde(default)]
    pub build_env: FeedEnv,
}

fn default_channel() -> Value {
    Value::Text("stable".into())
}

fn default_feed_out() -> PathBuf {
    "target/feed".into()
}

/// Names of build-time environment variables.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedEnv {
    /// Receives the feed URL.
    pub url: Option<String>,
    /// Receives the public key's base64 line.
    pub key: Option<String>,
}

/// A variant of the application, such as one with a test identifier that
/// installs beside the real one and takes nothing from it. On Linux its
/// desktop entry, icons and MIME file are named after its id, so both can be
/// integrated at once.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variant {
    /// Another identifier.
    pub id: Option<String>,
    /// Another name.
    pub name: Option<String>,
    /// Every document type's rank, such as `alternate`.
    pub document_rank: Option<Rank>,
    /// `imported` turns the exported type declarations into imported ones,
    /// so the variant claims no types (on Linux: declares no MIME types).
    pub types: Option<TypeOwnership>,
    /// Other URL schemes; `[]` registers none.
    pub url_schemes: Option<Vec<String>>,
    /// More Info.plist keys.
    #[serde(default)]
    pub plist: toml::Table,
    /// Other values for `[vars]`.
    #[serde(default)]
    pub vars: BTreeMap<String, Value>,
}

/// Whether type declarations are the application's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TypeOwnership {
    /// As declared.
    Exported,
    /// Exported declarations become imported ones.
    Imported,
}
