//! Publishing a release into the update feed, from the config's `[feed]`.
//!
//! The feed folder ([`crate::config::Feed::out`]) receives the artifacts an
//! installed copy downloads (the macOS update archive, the Windows installer,
//! the AppImage), `latest.json` and its signature, and is uploaded as it is.
//! A `.deb` and a pacman package are only announced: their entries point at
//! the copy in the apt or pacman repository, and the package is never copied
//! into the feed.

use crate::error::io;
use crate::linux::{deb_arch, deb_package, deb_version, pacman_arch};
use crate::{Error, Project, Result};
use roadcase_update::sign::{Part, has_password, password_from_env, publish_with_password};
use roadcase_update::{Kind, Manifest};
use std::path::{Path, PathBuf};

/// What was published.
#[derive(Debug, Clone)]
pub struct Published {
    /// The feed folder, ready to upload: artifacts first, `latest.json` last.
    pub folder: PathBuf,
    /// The manifest now in it.
    pub manifest: Manifest,
}

/// The secret key: `[feed] secret_key`, or else `ROADCASE_UPDATE_SECRET_KEY`,
/// then `CC_UPDATE_SECRET_KEY`. `None` when there is none, and nothing is
/// published.
///
/// # Errors
///
/// [`Error::Config`] without `[feed]`; [`Error::Value`].
pub fn secret_key(project: &Project) -> Result<Option<String>> {
    let feed = feed(project)?;
    let configured = match &feed.secret_key {
        Some(value) => Some(project.context.resolve(value, "feed.secret_key")?),
        None => None,
    }
    .filter(|key| !key.trim().is_empty());
    Ok(configured.or_else(|| {
        let environment = project.context.environment();
        [
            roadcase_update::sign::SECRET_KEY_ENV,
            roadcase_update::sign::LEGACY_SECRET_KEY_ENV,
        ]
        .into_iter()
        .find_map(|name| environment.get(name))
    }))
}

fn feed(project: &Project) -> Result<&crate::config::Feed> {
    project
        .config
        .feed
        .as_ref()
        .ok_or_else(|| Error::Config("the config has no [feed]".into()))
}

/// Writes `parts` into the feed folder with `latest.json` and its signature,
/// for the config's version and notes. A manifest already there for the same
/// version keeps its other platforms, so sync the folder from the host first
/// when another machine already added its part. Answers `None`, having
/// written nothing, when there is no secret key or nothing to publish.
///
/// # Errors
///
/// [`Error::Config`] without `[feed]`, or when the key has a password and
/// `ROADCASE_UPDATE_KEY_PASSWORD` is not set; [`Error::Command`] when signing
/// fails; [`Error::Value`].
pub fn publish(project: &Project, parts: &[Part]) -> Result<Option<Published>> {
    let feed = feed(project)?;
    let context = &project.context;
    let Some(secret) = secret_key(project)? else {
        return Ok(None);
    };
    if parts.is_empty() {
        return Ok(None);
    }
    let failed = |error: roadcase_update::Error| Error::Command(format!("publishing: {error}"));
    let password = if has_password(&secret).map_err(failed)? {
        Some(
            context
                .environment()
                .get(roadcase_update::sign::PASSWORD_ENV)
                .or_else(password_from_env)
                .ok_or_else(|| {
                    Error::Config(format!(
                        "the update key has a password: set {}",
                        roadcase_update::sign::PASSWORD_ENV
                    ))
                })?,
        )
    } else {
        None
    };
    let version = roadcase_update::Version::parse(context.version())
        .map_err(|error| Error::Config(format!("app.version: {error}")))?;
    let notes = match &feed.notes {
        Some(value) => Some(context.resolve(value, "feed.notes")?),
        None => None,
    }
    .filter(|notes| !notes.trim().is_empty());
    let folder = context.path(&feed.out)?;
    let manifest = publish_with_password(
        &folder,
        &version,
        notes.as_deref(),
        parts,
        &secret,
        password.as_deref(),
    )
    .map_err(failed)?;
    Ok(Some(Published { folder, manifest }))
}

fn arch(project: &Project) -> &str {
    project.context.get("arch").unwrap_or_default()
}

/// The macOS update archive, under `macos-<arch>`.
pub fn macos_part(project: &Project, archive: &Path) -> Part {
    Part {
        target: format!("macos-{}", arch(project)),
        kind: Kind::AppTarGz,
        path: archive.to_path_buf(),
        url: None,
    }
}

/// The Windows installer, under `windows-x86_64`, copied beside itself under
/// its feed name (`[windows.nsis] feed_name`: versioned, so that a manifest
/// that still names an older installer finds it).
///
/// # Errors
///
/// [`Error::Config`] without `[windows]`; [`Error::Value`]; [`Error::Io`].
pub fn windows_part(project: &Project, installer: &Path) -> Result<Part> {
    let name = project.context.fill(&project.windows()?.nsis.feed_name)?;
    let named = installer.with_file_name(name);
    if named != installer {
        std::fs::copy(installer, &named).map_err(io(installer))?;
    }
    Ok(Part {
        target: format!("windows-{}", arch(project)),
        kind: Kind::Nsis,
        path: named,
        url: None,
    })
}

/// The AppImage, under `linux-<arch>`.
pub fn appimage_part(project: &Project, appimage: &Path) -> Part {
    Part {
        target: format!("linux-{}", arch(project)),
        kind: Kind::AppImage,
        path: appimage.to_path_buf(),
        url: None,
    }
}

/// A path segment in a URL: everything but letters, digits and `-._~`
/// percent-encoded, so that a version's `+` survives every host.
fn segment(text: &str) -> String {
    let mut encoded = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Where the apt repository publishes this release's `.deb`, as
/// `roadcase repo apt` keeps it:
/// `<repository>/pool/<component>/<p>/<package>/<package>_<version>_<arch>.deb`.
/// `None` when the config names no repository (`[linux.deb] repository`, or
/// `apt_source.url`).
///
/// # Errors
///
/// [`Error::Config`] without `[linux.deb]`; [`Error::Value`].
pub fn deb_url(project: &Project) -> Result<Option<String>> {
    let context = &project.context;
    let deb = project
        .linux()?
        .deb
        .as_ref()
        .ok_or_else(|| Error::Config("the config has no [linux.deb]".into()))?;
    let repository = match (&deb.repository, &deb.apt_source) {
        (Some(repository), _) => context.fill(repository)?,
        (None, Some(source)) => context.fill(&source.url)?,
        (None, None) => return Ok(None),
    };
    let component = deb
        .apt_source
        .as_ref()
        .and_then(|source| source.components.first())
        .map_or("main", String::as_str);
    let package = deb_package(project)?;
    // Debian's pool: the first letter, or the first four of a lib package.
    let prefix = if package.starts_with("lib") && package.len() > 3 {
        &package[..4]
    } else {
        &package[..1.min(package.len())]
    };
    let file = format!(
        "{package}_{}_{}.deb",
        deb_version(context.version(), &context.fill(&deb.revision)?),
        deb_arch(arch(project))?
    );
    Ok(Some(format!(
        "{}/pool/{component}/{prefix}/{package}/{}",
        repository.trim_end_matches('/'),
        segment(&file)
    )))
}

/// The `.deb`, announced under `linux-<arch>-deb` at [`deb_url`]; `None`
/// when the config names no repository.
///
/// # Errors
///
/// As [`deb_url`].
pub fn deb_part(project: &Project, package: &Path) -> Result<Option<Part>> {
    Ok(deb_url(project)?.map(|url| Part {
        target: format!("linux-{}-deb", arch(project)),
        kind: Kind::Deb,
        path: package.to_path_buf(),
        url: Some(url),
    }))
}

/// Where the pacman repository publishes a package, as `roadcase repo
/// pacman` keeps it: `<repository>/<arch>/<file>`. `None` without
/// `[linux.pacman] repository`.
///
/// # Errors
///
/// [`Error::Config`] without `[linux.pacman]`; [`Error::Value`].
pub fn pacman_url(project: &Project, package: &Path) -> Result<Option<String>> {
    let pacman = project
        .linux()?
        .pacman
        .as_ref()
        .ok_or_else(|| Error::Config("the config has no [linux.pacman]".into()))?;
    let Some(repository) = &pacman.repository else {
        return Ok(None);
    };
    let file = package
        .file_name()
        .ok_or_else(|| Error::Missing(format!("{} has no name", package.display())))?
        .to_string_lossy();
    Ok(Some(format!(
        "{}/{}/{}",
        project.context.fill(repository)?.trim_end_matches('/'),
        pacman_arch(arch(project))?,
        segment(&file)
    )))
}

/// The pacman package, announced under `linux-<arch>-pacman` at
/// [`pacman_url`]; `None` when the config names no repository.
///
/// # Errors
///
/// As [`pacman_url`].
pub fn pacman_part(project: &Project, package: &Path) -> Result<Option<Part>> {
    Ok(pacman_url(project, package)?.map(|url| Part {
        target: format!("linux-{}-pacman", arch(project)),
        kind: Kind::Pacman,
        path: package.to_path_buf(),
        url: Some(url),
    }))
}
