//! `roadcase`: packages desktop applications from a `roadcase.toml`, and makes
//! the update key and publishes releases into a feed folder that is uploaded
//! as it is.

use clap::{Parser, Subcommand, ValueEnum};
use roadcase_bundle::{Options, Project, commands, linux, macos, publish, windows};
use roadcase_update::sign::{
    PASSWORD_ENV, Part, generate, generate_with_password, has_password, password_from_env,
    publish_with_password, secret_from_env,
};
use roadcase_update::{Kind, MANIFEST, Manifest, PublicKey, SIGNATURE, Version};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "roadcase", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Writes update.key (secret: with no password, keep it in a secret store)
    /// and update.pub (compiled into the application) into a folder.
    Keygen {
        /// The folder. A key already there is never replaced: a new key locks
        /// out every installed copy that holds the old one.
        folder: PathBuf,
        /// Encrypts update.key with a password, in minisign's format, for a
        /// key kept in a file. The password is asked for twice, or read from
        /// ROADCASE_UPDATE_KEY_PASSWORD.
        #[arg(long)]
        password: bool,
    },
    /// Copies artifacts into a feed folder and writes latest.json and its
    /// signature there. A latest.json of the same version keeps its other
    /// targets.
    Publish {
        /// The release's version.
        #[arg(long)]
        version: Version,
        /// The feed folder.
        #[arg(long)]
        out: PathBuf,
        /// What changed.
        #[arg(long)]
        notes: Option<String>,
        /// update.key. Without it, the key is read from
        /// ROADCASE_UPDATE_SECRET_KEY, or else CC_UPDATE_SECRET_KEY.
        #[arg(long)]
        key: Option<PathBuf>,
        /// The environment variable holding an encrypted key's password.
        /// Without it, ROADCASE_UPDATE_KEY_PASSWORD, or else a prompt in a
        /// terminal.
        #[arg(long, value_name = "NAME")]
        password_env: Option<String>,
        /// <target>=<kind>=<file>: targets such as windows-x86_64,
        /// macos-aarch64, linux-x86_64; kinds nsis, app-tar-gz, appimage.
        /// A .deb is announced as linux-<arch>-deb=deb=<file>, a pacman
        /// package as linux-<arch>-pacman=pacman=<file>: copies installed
        /// from them learn of the release, and update through apt or pacman.
        /// Repeats.
        #[arg(long = "artifact", required = true)]
        artifacts: Vec<String>,
        /// <target>=<url>: where that artifact is published, when not in the
        /// feed folder, such as a .deb's place in its apt repository. The
        /// file is then measured and not copied. Repeats.
        #[arg(long = "url")]
        urls: Vec<String>,
    },
    /// Checks latest.json in a feed folder against the key, and the
    /// artifacts beside it against latest.json.
    Verify {
        /// update.pub, or its base64 line.
        #[arg(long)]
        key: String,
        /// The feed folder.
        folder: PathBuf,
    },
    /// Reads and checks a roadcase.toml, and prints what it resolves to.
    Check {
        #[command(flatten)]
        project: ProjectArgs,
        /// The system to resolve it for.
        #[arg(long, value_enum, default_value_t = System::host())]
        system: System,
    },
    /// Assembles the macOS .app, unsigned, and prints its path and what will
    /// be signed with which entitlements; or, with --system linux, the
    /// AppDir an AppImage is made from.
    App {
        #[command(flatten)]
        project: ProjectArgs,
        /// The system to assemble for.
        #[arg(long, value_enum, default_value_t = System::host())]
        system: System,
        /// Where the .app or the AppDir goes.
        #[arg(long, default_value = "target/roadcase")]
        out: PathBuf,
        /// Runs the config's build commands first.
        #[arg(long)]
        build: bool,
    },
    /// Builds, packages, signs, notarizes and publishes a release for one
    /// system, on that system. With a [feed] and its secret key
    /// (ROADCASE_UPDATE_SECRET_KEY), it ends by writing the feed folder:
    /// the artifacts, latest.json and its signature.
    Release {
        #[command(flatten)]
        project: ProjectArgs,
        /// The system; the one this runs on when left out. A release is
        /// only ever made on its own system.
        #[arg(value_enum, default_value_t = System::host())]
        system: System,
        /// Where the artifacts go.
        #[arg(long, default_value = "target/roadcase")]
        out: PathBuf,
        /// Packages what is already built.
        #[arg(long)]
        skip_build: bool,
        /// On Linux, makes only these of the formats the config has
        /// (appimage, deb, pacman). Repeats; all of them by default.
        #[arg(long = "format", value_enum)]
        formats: Vec<LinuxFormat>,
    },
    /// Signed package repositories as static folders, uploaded as they are:
    /// apt and pacman repositories, and AUR -bin packages.
    #[command(subcommand)]
    Repo(Repo),
    /// A temporary macOS keychain for a signing certificate that arrives as a
    /// base64 .p12, for tools other than `roadcase release macos` (which
    /// imports CODESIGN_CERTIFICATE itself, and removes it afterwards).
    #[command(subcommand)]
    Keychain(Keychain),
}

#[derive(Subcommand)]
enum Keychain {
    /// Makes the keychain, imports the certificate for codesign to use
    /// without asking, puts the keychain on the search list, and prints its
    /// path: export CODESIGN_KEYCHAIN="$(roadcase keychain import)".
    Import {
        /// The environment variable holding the base64 .p12.
        #[arg(long, value_name = "NAME", default_value = "CODESIGN_CERTIFICATE")]
        certificate_env: String,
        /// The environment variable holding the .p12's password.
        #[arg(
            long,
            value_name = "NAME",
            default_value = "CODESIGN_CERTIFICATE_PASSWORD"
        )]
        password_env: String,
        /// The keychain's file. By default one in $RUNNER_TEMP, or the
        /// system's temporary folder.
        #[arg(long)]
        keychain: Option<PathBuf>,
    },
    /// Removes a keychain `keychain import` made, and takes it off the
    /// search list.
    Remove {
        /// The keychain's file, as `keychain import` printed it.
        keychain: PathBuf,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum LinuxFormat {
    Appimage,
    Deb,
    Pacman,
}

#[derive(Subcommand)]
enum Repo {
    /// Writes a repository signing key into a folder: signing.key (armored
    /// secret key: keep it in a secret store), key.asc (the public key,
    /// armored) and key.gpg (the same, binary, for apt's Signed-By).
    Keygen {
        /// The folder. A key already there is never replaced.
        folder: PathBuf,
        /// Who signs: Example Packages <packages@example.com>.
        #[arg(long)]
        name: String,
        /// Encrypts the secret key with a password, read from
        /// ROADCASE_REPO_SIGNING_KEY_PASSWORD or asked for twice.
        #[arg(long)]
        password: bool,
    },
    /// Adds .deb files to an apt repository folder and writes its index:
    /// pool/, dists/<suite>/ with Packages, Release, Release.gpg and
    /// InRelease, and key.asc and key.gpg. Sync the folder down from the host
    /// first, and up again after (with deletions: older versions beyond
    /// --keep are removed).
    Apt {
        /// The repository folder.
        folder: PathBuf,
        /// .deb files to add. Without any, the index is written again.
        #[arg(long = "add")]
        packages: Vec<PathBuf>,
        /// The suite.
        #[arg(long, default_value = "stable")]
        suite: String,
        /// The codename; the suite when absent.
        #[arg(long)]
        codename: Option<String>,
        /// The component the packages go into.
        #[arg(long, default_value = "main")]
        component: String,
        /// Release's Origin.
        #[arg(long)]
        origin: Option<String>,
        /// Release's Label.
        #[arg(long)]
        label: Option<String>,
        /// Release's Description.
        #[arg(long)]
        description: Option<String>,
        /// Versions kept of each package and architecture.
        #[arg(long, default_value_t = 3)]
        keep: usize,
        #[command(flatten)]
        key: RepoKey,
    },
    /// Adds pacman packages to a pacman repository folder and writes its
    /// databases: <arch>/<name>.db and <name>.files, signed, every package's
    /// .sig, and key.asc. Sync the folder down from the host first, and up
    /// again after (with deletions).
    Pacman {
        /// The repository folder: it holds one folder per architecture.
        folder: PathBuf,
        /// The repository's name: [<name>] in pacman.conf.
        #[arg(long)]
        name: String,
        /// Packages (*.pkg.tar.zst) to add. Without any, the databases are
        /// written again.
        #[arg(long = "add")]
        packages: Vec<PathBuf>,
        /// Versions kept of each package: the newest is listed, older ones
        /// stay for links that still name them.
        #[arg(long, default_value_t = 3)]
        keep: usize,
        #[command(flatten)]
        key: RepoKey,
    },
    /// Writes an AUR -bin package (PKGBUILD and .SRCINFO) for the pacman
    /// packages of one release, as published in a pacman repository.
    /// Publishing is a git push to the AUR, made by you or your CI.
    Aur {
        /// The pacman packages, one per architecture.
        #[arg(required = true)]
        packages: Vec<PathBuf>,
        /// The pacman repository's URL: the folder that holds <arch>/.
        #[arg(long)]
        url: String,
        /// The folder the PKGBUILD and .SRCINFO go into.
        #[arg(long)]
        out: PathBuf,
        /// The # Maintainer: line.
        #[arg(long)]
        maintainer: Option<String>,
        /// The AUR package's name: <pkgname>-bin by default.
        #[arg(long)]
        pkgname: Option<String>,
    },
}

#[derive(clap::Args)]
struct RepoKey {
    /// The armored secret key's file. Without it, the key is read from
    /// ROADCASE_REPO_SIGNING_KEY.
    #[arg(long = "key")]
    file: Option<PathBuf>,
    /// The environment variable holding the key's password; by default
    /// ROADCASE_REPO_SIGNING_KEY_PASSWORD.
    #[arg(long, value_name = "NAME")]
    password_env: Option<String>,
}

impl RepoKey {
    fn load(&self) -> Result<roadcase_repo::key::SigningKey, String> {
        use roadcase_repo::key::{PASSWORD_ENV, SigningKey};
        match &self.file {
            Some(path) => {
                let armored = std::fs::read_to_string(path)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                let name = self.password_env.as_deref().unwrap_or(PASSWORD_ENV);
                let password = std::env::var(name).ok().filter(|value| !value.is_empty());
                SigningKey::parse(&armored, password.as_deref()).map_err(text)
            }
            None => SigningKey::from_env(self.password_env.as_deref()).map_err(text),
        }
    }
}

#[derive(clap::Args)]
struct ProjectArgs {
    /// The config.
    #[arg(long, default_value = "roadcase.toml")]
    config: PathBuf,
    /// A variant from [variant.<name>].
    #[arg(long)]
    variant: Option<String>,
}

#[derive(Clone, Copy, ValueEnum)]
enum System {
    Macos,
    Windows,
    Linux,
}

impl System {
    fn host() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Macos
        }
    }

    fn options(self) -> Options {
        match self {
            Self::Macos => Options::macos(),
            Self::Windows => Options::windows(),
            Self::Linux => Options::linux(),
        }
    }
}

impl ProjectArgs {
    fn load(&self, system: System) -> Result<Project, String> {
        Project::load(
            &self.config,
            system.options().with_variant(self.variant.as_deref()),
        )
        .map_err(text)
    }
}

fn main() -> ExitCode {
    let done = match Cli::parse().command {
        Command::Keygen { folder, password } => keygen(&folder, password),
        Command::Publish {
            version,
            out,
            notes,
            key,
            password_env,
            artifacts,
            urls,
        } => release(
            &version,
            &out,
            notes.as_deref(),
            key.as_deref(),
            password_env.as_deref(),
            &artifacts,
            &urls,
        ),
        Command::Verify { key, folder } => verify(&key, &folder),
        Command::Check { project, system } => check(&project, system),
        Command::App {
            project,
            system,
            out,
            build,
        } => app(&project, system, &out, build),
        Command::Release {
            project,
            system,
            out,
            skip_build,
            formats,
        } => package(&project, system, &out, skip_build, &formats),
        Command::Repo(command) => repo(command),
        Command::Keychain(command) => keychain(command),
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("roadcase: {message}");
            ExitCode::FAILURE
        }
    }
}

fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// A password from the terminal, without echo.
fn prompt(question: &str) -> Result<String, String> {
    rpassword::prompt_password(question).map_err(|error| format!("reading the password: {error}"))
}

/// The password for a new key: from ROADCASE_UPDATE_KEY_PASSWORD, or asked
/// for twice in a terminal.
fn new_password() -> Result<String, String> {
    if let Some(password) = password_from_env() {
        return Ok(password);
    }
    if !std::io::stdin().is_terminal() {
        return Err(format!(
            "--password needs a terminal to ask for the password, or {PASSWORD_ENV}"
        ));
    }
    let password = prompt("Password for update.key: ")?;
    if password.is_empty() {
        return Err("the password is empty".into());
    }
    if prompt("The same password again: ")? != password {
        return Err("the passwords differ".into());
    }
    Ok(password)
}

/// The password for an encrypted key: from the variable `--password-env`
/// names, or ROADCASE_UPDATE_KEY_PASSWORD, or asked for in a terminal.
fn key_password(named: Option<&str>) -> Result<String, String> {
    if let Some(name) = named {
        return std::env::var(name)
            .ok()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("--password-env {name}: {name} is not set"));
    }
    if let Some(password) = password_from_env() {
        return Ok(password);
    }
    if !std::io::stdin().is_terminal() {
        return Err(format!(
            "the secret key has a password: set {PASSWORD_ENV}, or name the variable that holds it with --password-env"
        ));
    }
    prompt("Password for the secret key: ")
}

fn keygen(folder: &Path, password: bool) -> Result<(), String> {
    let secret = folder.join("update.key");
    if secret.exists() {
        return Err(format!(
            "{} exists: a new key would lock out every installed application",
            secret.display()
        ));
    }
    let keys = if password {
        generate_with_password(&new_password()?)
    } else {
        generate()
    }
    .map_err(text)?;
    std::fs::create_dir_all(folder).map_err(text)?;
    std::fs::write(&secret, &keys.secret).map_err(text)?;
    std::fs::write(folder.join("update.pub"), &keys.public).map_err(text)?;
    println!("{}", keys.public);
    Ok(())
}

fn release(
    version: &Version,
    folder: &Path,
    notes: Option<&str>,
    key: Option<&Path>,
    password_env: Option<&str>,
    artifacts: &[String],
    urls: &[String],
) -> Result<(), String> {
    let secret = match key {
        Some(path) => {
            std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?
        }
        None => secret_from_env().ok_or_else(|| {
            "no --key, and neither ROADCASE_UPDATE_SECRET_KEY nor CC_UPDATE_SECRET_KEY is set"
                .to_owned()
        })?,
    };
    let mut parts = Vec::new();
    for artifact in artifacts {
        let mut fields = artifact.splitn(3, '=');
        let (Some(target), Some(kind), Some(path)) = (fields.next(), fields.next(), fields.next())
        else {
            return Err(format!(
                "--artifact {artifact} is not <target>=<kind>=<file>"
            ));
        };
        let kind = match kind {
            "nsis" => Kind::Nsis,
            "app-tar-gz" => Kind::AppTarGz,
            "appimage" => Kind::AppImage,
            "deb" => Kind::Deb,
            "pacman" => Kind::Pacman,
            other => {
                return Err(format!(
                    "{other} is not a kind: nsis, app-tar-gz, appimage, deb, pacman"
                ));
            }
        };
        parts.push(Part {
            target: target.to_owned(),
            kind,
            path: PathBuf::from(path),
            url: None,
        });
    }
    for url in urls {
        let Some((target, url)) = url.split_once('=') else {
            return Err(format!("--url {url} is not <target>=<url>"));
        };
        let part = parts
            .iter_mut()
            .find(|part| part.target == target)
            .ok_or_else(|| format!("--url {target}: there is no --artifact for {target}"))?;
        part.url = Some(url.to_owned());
    }
    let password = if has_password(&secret).map_err(text)? {
        Some(key_password(password_env)?)
    } else {
        None
    };
    let manifest =
        publish_with_password(folder, version, notes, &parts, &secret, password.as_deref())
            .map_err(text)?;
    let targets: Vec<_> = manifest.artifacts.keys().map(String::as_str).collect();
    println!(
        "{} holds {version} for {}",
        folder.display(),
        targets.join(", ")
    );
    Ok(())
}

fn verify(key: &str, folder: &Path) -> Result<(), String> {
    let key = std::fs::read_to_string(key).unwrap_or_else(|_| key.to_owned());
    let key = PublicKey::parse(&key).map_err(text)?;
    let bytes = std::fs::read(folder.join(MANIFEST)).map_err(text)?;
    let signature = std::fs::read_to_string(folder.join(SIGNATURE)).map_err(text)?;
    let manifest = Manifest::verified(&bytes, &signature, &key).map_err(text)?;
    println!("{MANIFEST}: {} is signed by this key", manifest.version);
    for (target, artifact) in &manifest.artifacts {
        let name = roadcase_update::sign::file_name(artifact);
        let path = folder.join(&name);
        if !path.is_file() {
            println!("{target}: {name} is not in this folder");
            continue;
        }
        let found = roadcase_update::sign::digest(&path).map_err(text)?;
        if found != (artifact.sha256.clone(), artifact.size) {
            return Err(format!("{target}: {name} is not the file that was signed"));
        }
        println!("{target}: {name} matches");
    }
    Ok(())
}

fn check(args: &ProjectArgs, system: System) -> Result<(), String> {
    let project = args.load(system)?;
    let context = &project.context;
    println!("{} ({})", context.name(), context.id());
    println!(
        "version {}, build {}",
        context.version(),
        context.build_number()
    );
    if let Some(variant) = project.variant_name() {
        println!("variant {variant}");
    }
    let schemes = project.url_schemes();
    if !schemes.is_empty() {
        println!("URL schemes: {}", schemes.join(", "));
    }
    if let Some(macos) = &project.config.macos {
        println!(
            "macOS: {} program(s), {} extension(s), {} launch agent(s)",
            macos.programs.len(),
            macos.extensions.len(),
            macos.launch_agents.len()
        );
    }
    if project.config.windows.is_some() {
        println!(
            "Windows: {} installed as {}",
            context.name(),
            windows::executable_name(&project).map_err(text)?
        );
    }
    if let Some(linux) = &project.config.linux {
        let types = linux::handled_types(&project);
        println!(
            "Linux: usr/bin/{} and {} program(s), desktop entry {}.desktop{}",
            linux::executable_name(&project).map_err(text)?,
            linux.programs.len(),
            context.id(),
            if types.is_empty() {
                String::new()
            } else {
                format!(" for {}", types.join(", "))
            }
        );
    }
    if let Some(linux_config) = &project.config.linux {
        if linux_config.deb.is_some() {
            let name = linux::deb_package(&project).map_err(text)?;
            println!("Linux .deb: {name}, installed in /opt/{name}");
        }
        if linux_config.pacman.is_some() {
            let name = linux::pacman_package(&project).map_err(text)?;
            println!("Linux pacman package: {name}, installed in /opt/{name}");
        }
    }
    for (name, value) in commands::feed_environment(&project).map_err(text)? {
        println!("build env {name}={value}");
    }
    println!("ok");
    Ok(())
}

fn app(args: &ProjectArgs, system: System, out: &Path, build: bool) -> Result<(), String> {
    let project = args.load(system)?;
    if build {
        commands::build(&project, matches!(system, System::Macos)).map_err(text)?;
    }
    match system {
        System::Macos => {}
        System::Linux => {
            let app = linux::assemble(&project, out).map_err(text)?;
            for path in &app.symbols {
                eprintln!("symbols of {} go into a .debug file", path.display());
            }
            println!("{}", app.path.display());
            return Ok(());
        }
        System::Windows => {
            return Err("Windows has no bundle to assemble: use release windows".into());
        }
    }
    let app = macos::assemble(&project, out).map_err(text)?;
    for step in &app.signing {
        eprintln!(
            "sign {}{}{}",
            step.path.display(),
            step.entitlements
                .as_ref()
                .map(|path| format!(" with {}", path.display()))
                .unwrap_or_default(),
            step.identifier
                .as_ref()
                .map(|identifier| format!(" as {identifier}"))
                .unwrap_or_default(),
        );
    }
    println!("{}", app.path.display());
    Ok(())
}

/// Writes the feed for what a release made, and says what happened.
fn publish_parts(project: &Project, parts: &[Part]) -> Result<(), String> {
    if project.config.feed.is_none() {
        return Ok(());
    }
    match publish::publish(project, parts).map_err(text)? {
        Some(published) => {
            let targets: Vec<_> = published
                .manifest
                .artifacts
                .keys()
                .map(String::as_str)
                .collect();
            eprintln!(
                "published {} for {} in {}: upload the folder as it is, latest.json last",
                published.manifest.version,
                targets.join(", "),
                published.folder.display()
            );
        }
        None if parts.is_empty() => eprintln!("not published: nothing made here goes in the feed"),
        None => eprintln!(
            "not published: there is no update key (feed.secret_key, or ROADCASE_UPDATE_SECRET_KEY)"
        ),
    }
    Ok(())
}

fn package(
    args: &ProjectArgs,
    system: System,
    out: &Path,
    skip_build: bool,
    formats: &[LinuxFormat],
) -> Result<(), String> {
    if matches!(system, System::Linux) && !cfg!(target_os = "linux") {
        return Err(
            "Linux releases are made on Linux, never cross-built: `roadcase app --system linux` assembles the AppDir here"
                .into(),
        );
    }
    if matches!(system, System::Macos) && !cfg!(target_os = "macos") {
        return Err(
            "macOS releases are made on a Mac, never cross-built: `roadcase app --system macos` assembles the unsigned bundle here"
                .into(),
        );
    }
    if !matches!(system, System::Linux) && !formats.is_empty() {
        return Err("--format is for Linux".into());
    }
    let mut project = args.load(system)?;
    match system {
        System::Macos => {
            // Before the build: its commands may name the signing team.
            let signing = macos::Signing::prepare(&mut project).map_err(text)?;
            let made = (|| {
                if !skip_build {
                    commands::build(&project, true).map_err(text)?;
                }
                let release = macos::release(&project, &signing.identity, out).map_err(text)?;
                let mut parts = Vec::new();
                if let Some(archive) = &release.update_archive {
                    parts.push(publish::macos_part(&project, archive));
                }
                publish_parts(&project, &parts)?;
                Ok::<_, String>(release)
            })();
            // The temporary keychain goes, whatever happened.
            let removed = signing.finish().map_err(text);
            let release = made?;
            removed?;
            if project.macos().map_err(text)?.notarize.is_some() && !release.notarized {
                eprintln!(
                    "not notarized: the signature is ad hoc; set CODESIGN_IDENTITY to a Developer ID Application identity"
                );
            }
            if let Some(symbols) = &release.symbols {
                eprintln!(
                    "symbols: {}",
                    symbols.zip.as_ref().unwrap_or(&symbols.folder).display()
                );
            }
            println!("{}", release.app.path.display());
            for artifact in release.dmg.iter().chain(&release.update_archive) {
                println!("{}", artifact.display());
            }
        }
        System::Linux => {
            if !skip_build {
                commands::build(&project, false).map_err(text)?;
            }
            let config = project.linux().map_err(text)?;
            let app = linux::assemble(&project, out).map_err(text)?;
            if !app.symbols.is_empty() {
                let zip = linux::symbols(&project, &app, out).map_err(text)?;
                eprintln!("symbols: {}", zip.display());
            }
            let wanted = |format, configured: bool| -> Result<bool, String> {
                if formats.is_empty() {
                    return Ok(configured);
                }
                if formats.contains(&format) && !configured {
                    let table = match format {
                        LinuxFormat::Appimage => "appimage",
                        LinuxFormat::Deb => "deb",
                        LinuxFormat::Pacman => "pacman",
                    };
                    return Err(format!(
                        "--format {table}: the config has no [linux.{table}]"
                    ));
                }
                Ok(formats.contains(&format))
            };
            let mut artifacts = Vec::new();
            let mut parts = Vec::new();
            if wanted(LinuxFormat::Appimage, config.appimage.is_some())? {
                let appimage = linux::appimage(&project, &app, out).map_err(text)?;
                parts.push(publish::appimage_part(&project, &appimage));
                artifacts.push(appimage);
            }
            if wanted(LinuxFormat::Deb, config.deb.is_some())? {
                let deb = linux::deb(&project, &app, out).map_err(text)?;
                match publish::deb_part(&project, &deb).map_err(text)? {
                    Some(part) => parts.push(part),
                    None if project.config.feed.is_some() => eprintln!(
                        "the .deb is not announced in the feed: set linux.deb.repository (or apt_source) to its apt repository"
                    ),
                    None => {}
                }
                artifacts.push(deb);
            }
            if wanted(LinuxFormat::Pacman, config.pacman.is_some())? {
                let package = linux::pacman(&project, &app, out).map_err(text)?;
                match publish::pacman_part(&project, &package).map_err(text)? {
                    Some(part) => parts.push(part),
                    None if project.config.feed.is_some() => eprintln!(
                        "the pacman package is not announced in the feed: set linux.pacman.repository to its pacman repository"
                    ),
                    None => {}
                }
                artifacts.push(package);
            }
            for artifact in &artifacts {
                println!("{}", artifact.display());
            }
            publish_parts(&project, &parts)?;
        }
        System::Windows => {
            if !skip_build {
                commands::build(&project, false).map_err(text)?;
            }
            let installer = windows::installer(&project, out).map_err(text)?;
            if !installer.signed && project.windows().map_err(text)?.sign.is_some() {
                eprintln!(
                    "not signed: signing runs on Windows, with the variable windows.sign.when_env names set"
                );
            }
            let part = publish::windows_part(&project, &installer.path).map_err(text)?;
            publish_parts(&project, &[part])?;
            println!("{}", installer.path.display());
        }
    }
    Ok(())
}

fn keychain(command: Keychain) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("keychains are a Mac's".into());
    }
    match command {
        Keychain::Import {
            certificate_env,
            password_env,
            keychain,
        } => {
            let certificate = std::env::var(&certificate_env)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| format!("{certificate_env} is not set: it holds the base64 .p12"))?;
            let password = std::env::var(&password_env).unwrap_or_default();
            let path = keychain.unwrap_or_else(macos::TemporaryKeychain::default_path);
            let made =
                macos::TemporaryKeychain::import(&certificate, &password, &path).map_err(text)?;
            println!("{}", made.path().display());
            // It stays for the tools that follow: `keychain remove` takes it away.
            std::mem::forget(made);
            Ok(())
        }
        Keychain::Remove { keychain } => macos::remove_keychain(&keychain).map_err(text),
    }
}

fn repo(command: Repo) -> Result<(), String> {
    use roadcase_repo::{apt, aur, key, pacman};
    match command {
        Repo::Keygen {
            folder,
            name,
            password,
        } => {
            let secret = folder.join("signing.key");
            if secret.exists() {
                return Err(format!(
                    "{} exists: a new key would make every installed copy refuse the repository",
                    secret.display()
                ));
            }
            let password = if password {
                Some(new_repo_password()?)
            } else {
                None
            };
            let pair = key::generate(&name, password.as_deref()).map_err(text)?;
            std::fs::create_dir_all(&folder).map_err(text)?;
            std::fs::write(&secret, &pair.secret).map_err(text)?;
            std::fs::write(folder.join("key.asc"), &pair.public).map_err(text)?;
            std::fs::write(folder.join("key.gpg"), &pair.public_binary).map_err(text)?;
            println!("{}", pair.fingerprint);
            Ok(())
        }
        Repo::Apt {
            folder,
            packages,
            suite,
            codename,
            component,
            origin,
            label,
            description,
            keep,
            key,
        } => {
            let key = key.load()?;
            for path in apt::add(&folder, &component, &packages).map_err(text)? {
                eprintln!("added {}", path.display());
            }
            let options = apt::Apt {
                suite,
                codename,
                origin,
                label,
                description,
                keep,
            };
            let report = apt::index(&folder, &options, &key).map_err(text)?;
            for path in &report.removed {
                eprintln!("removed {}", path.display());
            }
            for (component, package, version, arch) in &report.listed {
                println!("{component} {package} {version} {arch}");
            }
            Ok(())
        }
        Repo::Pacman {
            folder,
            name,
            packages,
            keep,
            key,
        } => {
            let key = key.load()?;
            for path in pacman::add(&folder, &packages).map_err(text)? {
                eprintln!("added {}", path.display());
            }
            let report =
                pacman::index(&folder, &pacman::Pacman { name, keep }, &key).map_err(text)?;
            for path in &report.removed {
                eprintln!("removed {}", path.display());
            }
            for (arch, package, version) in &report.listed {
                println!("{arch} {package} {version}");
            }
            Ok(())
        }
        Repo::Aur {
            packages,
            url,
            out,
            maintainer,
            pkgname,
        } => {
            let generated = aur::generate(
                &packages,
                &aur::Aur {
                    url,
                    maintainer,
                    pkgname,
                },
            )
            .map_err(text)?;
            std::fs::create_dir_all(&out).map_err(text)?;
            std::fs::write(out.join("PKGBUILD"), &generated.pkgbuild).map_err(text)?;
            std::fs::write(out.join(".SRCINFO"), &generated.srcinfo).map_err(text)?;
            println!("{}", generated.pkgname);
            Ok(())
        }
    }
}

/// The password for a new repository key: from
/// ROADCASE_REPO_SIGNING_KEY_PASSWORD, or asked for twice in a terminal.
fn new_repo_password() -> Result<String, String> {
    use roadcase_repo::key::PASSWORD_ENV;
    if let Some(password) = std::env::var(PASSWORD_ENV)
        .ok()
        .filter(|value| !value.is_empty())
    {
        return Ok(password);
    }
    if !std::io::stdin().is_terminal() {
        return Err(format!(
            "--password needs a terminal to ask for the password, or {PASSWORD_ENV}"
        ));
    }
    let password = prompt("Password for signing.key: ")?;
    if password.is_empty() {
        return Err("the password is empty".into());
    }
    if prompt("The same password again: ")? != password {
        return Err("the passwords differ".into());
    }
    Ok(password)
}
