//! What follows assembly in a macOS release: dSYMs and strip, notarization
//! and stapling, the disk image, and the archive the updater installs.

use super::signing::{Identity, SignFlags, sign, sign_file_command, write_private};
use super::{AppBundle, assemble};
use crate::config::{Notarize, Notarized};
use crate::error::io;
use crate::process::Invocation;
use crate::{Error, Project, Result};
use std::path::{Path, PathBuf};

const XCRUN: &str = "/usr/bin/xcrun";

/// The symbols taken out of a bundle's programs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbols {
    /// The folder of dSYMs, `<out>/<name>.dSYMs`: one `<program>.dSYM` each.
    pub folder: PathBuf,
    /// The zip of them, when `[macos.symbols]` asks for one.
    pub zip: Option<PathBuf>,
}

/// The UUIDs in what `dwarfdump --uuid` prints, each with its architecture:
/// `UUID: 0123... (arm64) path` is `0123... (arm64)`.
pub fn parse_uuids(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            (words.next()? == "UUID:").then_some(())?;
            Some(format!("{} {}", words.next()?, words.next()?))
        })
        .collect()
}

fn uuids(path: &Path) -> Result<Vec<String>> {
    let said = Invocation::new(XCRUN)
        .args(["dwarfdump", "--uuid"])
        .path(path)
        .run()?;
    Ok(parse_uuids(&said))
}

/// Makes a dSYM of each executable in [`AppBundle::symbols`] with `dsymutil`,
/// before anything is stripped, in `<out>/<name>.dSYMs` (never inside the
/// bundle); strips the executable; and checks that the dSYM's UUID is its
/// stripped program's, so that a crash report's image UUID names it. With
/// `[macos.symbols]`, zips the dSYMs. Run it before signing.
///
/// # Errors
///
/// [`Error::Command`] when a tool fails or a dSYM is not its program's;
/// [`Error::Io`].
pub fn symbols(project: &Project, app: &AppBundle, out: &Path) -> Result<Symbols> {
    let context = &project.context;
    let folder = out.join(format!("{}.dSYMs", context.name()));
    if folder.exists() {
        std::fs::remove_dir_all(&folder).map_err(io(&folder))?;
    }
    std::fs::create_dir_all(&folder).map_err(io(&folder))?;
    for program in &app.symbols {
        let name = program
            .file_name()
            .ok_or_else(|| Error::Missing(format!("{} has no name", program.display())))?
            .to_string_lossy();
        let dsym = folder.join(format!("{name}.dSYM"));
        Invocation::new(XCRUN)
            .arg("dsymutil")
            .path(program)
            .arg("-o")
            .path(&dsym)
            .run_all()?;
        Invocation::new(XCRUN)
            .args(["strip", "-no_code_signature_warning"])
            .path(program)
            .run_all()?;
        let stripped = uuids(program)?;
        if stripped.is_empty() || stripped != uuids(&dsym)? {
            return Err(Error::Command(format!(
                "{} is not {name}'s: its UUID is not the stripped program's ({})",
                dsym.display(),
                if stripped.is_empty() {
                    "which has none".to_owned()
                } else {
                    stripped.join(", ")
                }
            )));
        }
    }
    let zip = match &project.macos()?.symbols {
        Some(symbols) => {
            let zip = out.join(context.fill(&symbols.zip)?);
            if zip.exists() {
                std::fs::remove_file(&zip).map_err(io(&zip))?;
            }
            Invocation::new("/usr/bin/ditto")
                .args(["-c", "-k", "--norsrc", "--noextattr", "--noacl"])
                .path(&folder)
                .path(&zip)
                .run_all()?;
            Some(zip)
        }
        None => None,
    };
    Ok(Symbols { folder, zip })
}

/// How `notarytool` signs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credentials {
    /// A profile stored with `xcrun notarytool store-credentials`.
    Profile(String),
    /// An App Store Connect API key.
    ApiKey {
        /// The `.p8` file.
        key: PathBuf,
        /// Its id.
        key_id: String,
        /// Its issuer; an individual key has none.
        issuer: Option<String>,
    },
    /// An Apple ID.
    AppleId {
        /// The Apple ID.
        apple_id: String,
        /// Its app-specific password.
        password: String,
        /// Its team.
        team_id: String,
    },
}

impl Credentials {
    /// Adds what `notarytool` takes them as.
    #[must_use]
    pub fn apply(&self, command: Invocation) -> Invocation {
        match self {
            Self::Profile(profile) => command.args(["--keychain-profile", profile]),
            Self::ApiKey {
                key,
                key_id,
                issuer,
            } => {
                let command = command.arg("--key").path(key).args(["--key-id", key_id]);
                match issuer {
                    Some(issuer) => command.args(["--issuer", issuer]),
                    None => command,
                }
            }
            Self::AppleId {
                apple_id,
                password,
                team_id,
            } => command
                .args(["--apple-id", apple_id, "--password"])
                .secret(password.as_str())
                .args(["--team-id", team_id]),
        }
    }
}

/// The credentials `[macos.notarize]` resolves to: the profile, else the API
/// key, else the Apple ID. An API key given as text is written, for its owner
/// only, to `scratch/notary-key.p8`, which [`NotaryKey`] removes.
///
/// # Errors
///
/// [`Error::Config`] when none is complete, saying what to set.
pub fn credentials(
    project: &Project,
    notarize: &Notarize,
    scratch: &Path,
) -> Result<(Credentials, NotaryKey)> {
    let context = &project.context;
    let value = |value: &Option<crate::config::Value>, what: &str| -> Result<Option<String>> {
        Ok(match value {
            Some(value) => Some(context.resolve(value, what)?).filter(|text| !text.is_empty()),
            None => None,
        })
    };
    if let Some(profile) = value(&notarize.profile, "macos.notarize.profile")? {
        return Ok((Credentials::Profile(profile), NotaryKey(None)));
    }
    let key = value(&notarize.key, "macos.notarize.key")?;
    let key_id = value(&notarize.key_id, "macos.notarize.key_id")?;
    if let (Some(key), Some(key_id)) = (key, key_id) {
        let issuer = value(&notarize.issuer, "macos.notarize.issuer")?;
        let (key, written) = if key.trim_start().starts_with("-----BEGIN") {
            let path = scratch.join("notary-key.p8");
            write_private(&path, key.as_bytes())?;
            (path.clone(), Some(path))
        } else {
            (context.base.join(key), None)
        };
        return Ok((
            Credentials::ApiKey {
                key,
                key_id,
                issuer,
            },
            NotaryKey(written),
        ));
    }
    let apple_id = value(&notarize.apple_id, "macos.notarize.apple_id")?;
    let password = value(&notarize.password, "macos.notarize.password")?;
    let team_id = value(&notarize.team_id, "macos.notarize.team_id")?
        .or_else(|| context.get("team").map(str::to_owned));
    match (apple_id, password, team_id) {
        (Some(apple_id), Some(password), Some(team_id)) => Ok((
            Credentials::AppleId {
                apple_id,
                password,
                team_id,
            },
            NotaryKey(None),
        )),
        _ => Err(Error::Config(
            "[macos.notarize] has no credentials: set NOTARY_PROFILE (a profile stored with \
             `xcrun notarytool store-credentials`), or NOTARY_KEY, NOTARY_KEY_ID and \
             NOTARY_ISSUER (an App Store Connect API key), or NOTARY_APPLE_ID, NOTARY_PASSWORD \
             and NOTARY_TEAM_ID"
                .into(),
        )),
    }
}

/// An API key file written for one run, removed when dropped.
#[derive(Debug)]
pub struct NotaryKey(Option<PathBuf>);

impl Drop for NotaryKey {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// `notarytool submit`, waiting for Apple's answer, as JSON.
pub fn submit_command(credentials: &Credentials, file: &Path) -> Invocation {
    credentials
        .apply(
            Invocation::new(XCRUN)
                .args(["notarytool", "submit"])
                .path(file),
        )
        .args(["--wait", "--output-format", "json"])
}

/// A string field of `notarytool`'s JSON answer.
pub fn json_field(json: &str, field: &str) -> Option<String> {
    let pattern = format!(r#""{}"\s*:\s*"([^"]*)""#, regex::escape(field));
    regex::Regex::new(&pattern)
        .ok()?
        .captures(json)
        .map(|found| found[1].to_owned())
}

/// Submits `file` (a zip of the application, or the disk image) and waits
/// until Apple accepts it.
///
/// # Errors
///
/// [`Error::Command`] when the submission fails or is not accepted, with
/// Apple's log of why.
pub fn notarize(credentials: &Credentials, file: &Path) -> Result<()> {
    let answer = submit_command(credentials, file).run()?;
    let status = json_field(&answer, "status").unwrap_or_default();
    if status == "Accepted" {
        return Ok(());
    }
    let log = json_field(&answer, "id")
        .and_then(|id| {
            credentials
                .apply(Invocation::new(XCRUN).args(["notarytool", "log", &id]))
                .run()
                .ok()
        })
        .unwrap_or_default();
    Err(Error::Command(format!(
        "notarization of {} was not accepted (status {}):\n{}\n{}",
        file.display(),
        if status.is_empty() {
            "unknown"
        } else {
            &status
        },
        answer.trim(),
        log.trim()
    )))
}

fn staple(path: &Path) -> Result<()> {
    Invocation::new(XCRUN)
        .args(["stapler", "staple"])
        .path(path)
        .run_all()
        .map(|_| ())
}

/// Notarizes and staples the application itself, and not only the disk
/// image: an update never passes through the image, and must open with no
/// network. Then asks Gatekeeper (`spctl`) whether it would open.
///
/// # Errors
///
/// [`Error::Command`].
pub fn notarize_app(credentials: &Credentials, app: &AppBundle) -> Result<()> {
    let zip = app.path.with_extension("notarize.zip");
    if zip.exists() {
        std::fs::remove_file(&zip).map_err(io(&zip))?;
    }
    Invocation::new("/usr/bin/ditto")
        .args(["-c", "-k", "--keepParent"])
        .path(&app.path)
        .path(&zip)
        .run_all()?;
    let done = notarize(credentials, &zip);
    let _ = std::fs::remove_file(&zip);
    done?;
    staple(&app.path)?;
    Invocation::new("/usr/sbin/spctl")
        .args(["--assess", "--type", "execute"])
        .path(&app.path)
        .run_all()?;
    Ok(())
}

/// Notarizes and staples a signed disk image, then asks Gatekeeper whether
/// it would open.
///
/// # Errors
///
/// [`Error::Command`].
pub fn notarize_dmg(credentials: &Credentials, dmg: &Path) -> Result<()> {
    notarize(credentials, dmg)?;
    staple(dmg)?;
    Invocation::new("/usr/sbin/spctl")
        .args([
            "--assess",
            "--type",
            "open",
            "--context",
            "context:primary-signature",
        ])
        .path(dmg)
        .run_all()?;
    Ok(())
}

/// `hdiutil create`, from a staging folder, compressed.
pub fn hdiutil_command(volume: &str, staging: &Path, dmg: &Path) -> Invocation {
    Invocation::new("/usr/bin/hdiutil")
        .args(["create", "-volname", volume, "-srcfolder"])
        .path(staging)
        .args(["-ov", "-format", "UDZO"])
        .path(dmg)
}

/// Makes the disk image with `hdiutil`: the application, and a link to
/// `/Applications` beside it unless `applications_link` is off; then signs
/// the image. Make it once the application is stapled.
///
/// # Errors
///
/// [`Error::Config`] without `[macos.dmg]`; [`Error::Command`]; [`Error::Io`].
pub fn dmg(project: &Project, app: &AppBundle, identity: &Identity, out: &Path) -> Result<PathBuf> {
    let context = &project.context;
    let config = project
        .macos()?
        .dmg
        .as_ref()
        .ok_or_else(|| Error::Config("the config has no [macos.dmg]".into()))?;
    let dmg = out.join(context.fill(&config.name)?);
    let volume = match &config.volume_name {
        Some(name) => context.fill(name)?,
        None => context.name().to_owned(),
    };
    let staging = out.join(format!(".{}.dmg.d", context.name()));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(io(&staging))?;
    }
    std::fs::create_dir_all(&staging).map_err(io(&staging))?;
    let made = (|| {
        let name = app
            .path
            .file_name()
            .ok_or_else(|| Error::Missing(format!("{} has no name", app.path.display())))?;
        Invocation::new("/usr/bin/ditto")
            .path(&app.path)
            .path(&staging.join(name))
            .run_all()?;
        if config.applications_link {
            applications_link(&staging.join("Applications"))?;
        }
        if dmg.exists() {
            std::fs::remove_file(&dmg).map_err(io(&dmg))?;
        }
        create_image(&hdiutil_command(&volume, &staging, &dmg))?;
        sign_file_command(identity, SignFlags::of(project, identity)?, &dmg).run_all()?;
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&staging);
    made.map(|()| dmg)
}

/// Runs `hdiutil create`, again when the system says the resource is busy:
/// it does now and then right after files were copied, on busy machines.
fn create_image(command: &Invocation) -> Result<()> {
    let mut tries = 0;
    loop {
        match command.run_all() {
            Err(Error::Command(text)) if tries < 4 && text.contains("Resource busy") => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
            other => return other.map(|_| ()),
        }
    }
}

#[cfg(unix)]
fn applications_link(link: &Path) -> Result<()> {
    std::os::unix::fs::symlink("/Applications", link).map_err(io(link))
}

#[cfg(not(unix))]
fn applications_link(_: &Path) -> Result<()> {
    Err(Error::Config("a disk image is made on a Mac".into()))
}

/// `tar` for the update archive. `COPYFILE_DISABLE` keeps `._` files out:
/// they would land inside the bundle and break its seal.
pub fn archive_command(app: &Path, archive: &Path) -> Result<Invocation> {
    let (Some(parent), Some(name)) = (app.parent(), app.file_name()) else {
        return Err(Error::Missing(format!("{} has no folder", app.display())));
    };
    Ok(Invocation::new("/usr/bin/tar")
        .arg("-czf")
        .path(archive)
        .arg("-C")
        .path(parent)
        .arg(name.to_string_lossy().into_owned())
        .env("COPYFILE_DISABLE", "1"))
}

/// Makes the `.app.tar.gz` the updater installs. Make it once the
/// application is stapled, so that an update opens with no network.
///
/// # Errors
///
/// [`Error::Config`] without `[macos.update_archive]`; [`Error::Command`].
pub fn update_archive(project: &Project, app: &AppBundle, out: &Path) -> Result<PathBuf> {
    let config = project
        .macos()?
        .update_archive
        .as_ref()
        .ok_or_else(|| Error::Config("the config has no [macos.update_archive]".into()))?;
    let archive = out.join(project.context.fill(&config.name)?);
    if archive.exists() {
        std::fs::remove_file(&archive).map_err(io(&archive))?;
    }
    archive_command(&app.path, &archive)?.run_all()?;
    Ok(archive)
}

/// What a macOS release made.
#[derive(Debug, Clone)]
pub struct Release {
    /// The signed application.
    pub app: AppBundle,
    /// The symbols, when a program keeps them.
    pub symbols: Option<Symbols>,
    /// The disk image, with `[macos.dmg]`.
    pub dmg: Option<PathBuf>,
    /// The archive the updater installs, with `[macos.update_archive]`.
    pub update_archive: Option<PathBuf>,
    /// Whether Apple notarized it. Never for an ad hoc signature, which
    /// nothing outside this machine trusts.
    pub notarized: bool,
}

/// The whole release after the build: assemble, dSYMs and strip, sign inside
/// out, notarize and staple the application, make, sign, notarize and staple
/// the disk image, and make the update archive from the stapled application.
/// An ad hoc identity signs and skips notarization.
///
/// # Errors
///
/// Those of each step.
pub fn release(project: &Project, identity: &Identity, out: &Path) -> Result<Release> {
    if !cfg!(target_os = "macos") {
        return Err(Error::Config(
            "a macOS release is made on a Mac, never cross-built: `roadcase app --system macos` \
             assembles the unsigned bundle here"
                .into(),
        ));
    }
    let macos = project.macos()?;
    let app = assemble(project, out)?;
    let symbols = if app.symbols.is_empty() {
        None
    } else {
        Some(symbols(project, &app, out)?)
    };
    sign(project, &app, identity)?;

    let notarizing = macos
        .notarize
        .as_ref()
        .filter(|_| !identity.is_ad_hoc())
        .map(|notarize| {
            credentials(project, notarize, out).map(|credentials| (notarize, credentials))
        })
        .transpose()?;
    let wanted = |what: Notarized| {
        notarizing
            .as_ref()
            .filter(|(notarize, _)| notarize.staple.contains(&what))
            .map(|(_, (credentials, _))| credentials)
    };
    if let Some(credentials) = wanted(Notarized::App) {
        notarize_app(credentials, &app)?;
    }
    let dmg = match &macos.dmg {
        Some(_) => {
            let dmg = dmg(project, &app, identity, out)?;
            if let Some(credentials) = wanted(Notarized::Dmg) {
                notarize_dmg(credentials, &dmg)?;
            }
            Some(dmg)
        }
        None => None,
    };
    let update_archive = match &macos.update_archive {
        Some(_) => Some(update_archive(project, &app, out)?),
        None => None,
    };
    Ok(Release {
        app,
        symbols,
        dmg,
        update_archive,
        notarized: notarizing.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuids_are_read_with_their_architecture() {
        let said = "UUID: 2B5A1C6E-0D3F-3C0A-9E57-0A1B2C3D4E5F (arm64) /out/App.app/Contents/MacOS/App\nUUID: 11111111-2222-3333-4444-555555555555 (x86_64) /out/App.app/Contents/MacOS/App\n";
        assert_eq!(
            parse_uuids(said),
            [
                "2B5A1C6E-0D3F-3C0A-9E57-0A1B2C3D4E5F (arm64)",
                "11111111-2222-3333-4444-555555555555 (x86_64)"
            ]
        );
        assert!(parse_uuids("warning: no debug symbols\n").is_empty());
    }

    #[test]
    fn notarytool_takes_each_kind_of_credentials() {
        let file = Path::new("/out/App.zip");
        assert_eq!(
            submit_command(&Credentials::Profile("release".into()), file).display(),
            "/usr/bin/xcrun notarytool submit /out/App.zip --keychain-profile release --wait --output-format json"
        );
        let key = Credentials::ApiKey {
            key: "/keys/AuthKey.p8".into(),
            key_id: "ABC123".into(),
            issuer: Some("6f1c".into()),
        };
        assert_eq!(
            submit_command(&key, file).display(),
            "/usr/bin/xcrun notarytool submit /out/App.zip --key /keys/AuthKey.p8 --key-id ABC123 --issuer 6f1c --wait --output-format json"
        );
        let apple = Credentials::AppleId {
            apple_id: "dev@example.com".into(),
            password: "abcd-efgh".into(),
            team_id: "ABCDE12345".into(),
        };
        let command = submit_command(&apple, file);
        assert!(command.arguments.contains(&"abcd-efgh".to_owned()));
        assert_eq!(
            command.display(),
            "/usr/bin/xcrun notarytool submit /out/App.zip --apple-id dev@example.com --password *** --team-id ABCDE12345 --wait --output-format json"
        );
    }

    #[test]
    fn the_answer_says_whether_apple_accepted() {
        let answer = "{\n  \"id\": \"2efe2717-52ef-43a5-96dc-0797e4ca1041\",\n  \"status\" : \"Invalid\",\n  \"message\": \"Processing complete\"\n}";
        assert_eq!(json_field(answer, "status").as_deref(), Some("Invalid"));
        assert_eq!(
            json_field(answer, "id").as_deref(),
            Some("2efe2717-52ef-43a5-96dc-0797e4ca1041")
        );
        assert_eq!(json_field(answer, "nope"), None);
    }

    #[test]
    fn the_image_and_the_archive_are_made_as_the_updater_needs() {
        assert_eq!(
            hdiutil_command(
                "Example Studio",
                Path::new("/out/.s"),
                Path::new("/out/E.dmg")
            )
            .display(),
            "/usr/bin/hdiutil create -volname \"Example Studio\" -srcfolder /out/.s -ov -format UDZO /out/E.dmg"
        );
        let archive = archive_command(
            Path::new("/out/Example Studio.app"),
            Path::new("/out/E.app.tar.gz"),
        )
        .unwrap();
        assert_eq!(
            archive.display(),
            "/usr/bin/tar -czf /out/E.app.tar.gz -C /out \"Example Studio.app\""
        );
        assert_eq!(archive.env, [("COPYFILE_DISABLE".into(), "1".into())]);
    }
}
