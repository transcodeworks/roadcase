//! Handing a downloaded release to what installs it.

use crate::manifest::Kind;
use crate::{Error, Result};
use std::path::Path;

/// What an installation needs to know of the release and the running copy.
// Some fields are read only by the installer of one system.
#[allow(dead_code)]
pub(crate) struct Release<'a> {
    pub kind: Kind,
    pub sha256: &'a str,
    /// The Apple team the new bundle must be signed by.
    pub team: Option<&'a str>,
    /// The running version, as the application gave it.
    pub running: &'a str,
    /// The artifact's file name in the feed: the last segment of its URL.
    pub artifact: &'a str,
}

/// Starts the installation of the artifact at `path`. It finishes once this
/// process has left.
pub(crate) fn run(path: &Path, release: &Release<'_>, relaunch: &[&str]) -> Result<()> {
    let plain = |argument: &&str| {
        !argument.is_empty()
            && argument
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '=' | '.' | ':'))
    };
    if !relaunch.iter().all(plain) {
        return Err(Error::Unsupported(
            "an argument to start again with has characters an installer may not pass on".into(),
        ));
    }
    match release.kind {
        Kind::Nsis => nsis(path, relaunch),
        Kind::AppTarGz => app(path, relaunch, release.team),
        Kind::AppImage => appimage(path, relaunch, release),
        Kind::Deb => Err(Error::ManagedBySystem {
            manager: crate::PackageManager::Apt,
        }),
        Kind::Pacman => Err(Error::ManagedBySystem {
            manager: crate::PackageManager::Pacman,
        }),
        Kind::Unknown => Err(Error::Unsupported("its kind is not known".into())),
    }
}

/// NSIS's own flag for a silent install, which an update always passes.
pub const NSIS_SILENT: &str = "/S";
/// The flag that tells an application's installer it runs as an update.
pub const NSIS_UPDATE: &str = "/UPDATE";
/// The flag whose value, to the end of the line, is the arguments to start
/// the application again with.
pub const NSIS_RELAUNCH: &str = "/RELAUNCH=";

/// What an NSIS installer is started with, after its path. `/S` is NSIS's
/// own; `/UPDATE` and `/RELAUNCH=` are this crate's convention, read by the
/// application's script. `/RELAUNCH=` comes last and unquoted, since its value
/// runs to the end of the line.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn nsis_arguments(relaunch: &[&str]) -> String {
    let mut arguments = format!("{NSIS_SILENT} {NSIS_UPDATE}");
    if !relaunch.is_empty() {
        arguments.push(' ');
        arguments.push_str(NSIS_RELAUNCH);
        arguments.push_str(&relaunch.join(" "));
    }
    arguments
}

#[cfg(windows)]
fn nsis(path: &Path, relaunch: &[&str]) -> Result<()> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new(path)
        // As written: the standard quoting would wrap the last argument.
        .raw_arg(nsis_arguments(relaunch))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}

#[cfg(not(windows))]
fn nsis(_: &Path, _: &[&str]) -> Result<()> {
    Err(Error::Unsupported("it is an installer for Windows".into()))
}

/// Replaces the running AppImage, `$APPIMAGE`, and starts it again once this
/// process has left.
#[cfg(target_os = "linux")]
fn appimage(path: &Path, relaunch: &[&str], release: &Release<'_>) -> Result<()> {
    crate::appimage::install(
        path,
        release.sha256,
        std::env::var_os("APPIMAGE").map(std::path::PathBuf::from),
        release.running,
        release.artifact,
        relaunch,
        std::process::id(),
    )
}

#[cfg(not(target_os = "linux"))]
fn appimage(_: &Path, _: &[&str], _: &Release<'_>) -> Result<()> {
    Err(Error::Unsupported("it is an application for Linux".into()))
}

#[cfg(not(target_os = "macos"))]
fn app(_: &Path, _: &[&str], _: Option<&str>) -> Result<()> {
    Err(Error::Unsupported("it is an application for macOS".into()))
}

/// Waits for the application to leave, swaps the bundles, and opens the new
/// one. The old bundle comes back if the new one cannot take its place.
#[cfg(target_os = "macos")]
const SWAP: &str = r#"
pid="$1"; old="$2"; new="$3"; backup="$4"; work="$5"; shift 5
tries=0
while kill -0 "$pid" 2>/dev/null; do
  tries=$((tries + 1))
  [ "$tries" -gt 300 ] && exit 1
  sleep 0.2
done
if mv "$old" "$backup"; then
  if mv "$new" "$old"; then rm -rf "$backup"; else mv "$backup" "$old"; fi
fi
rm -rf "$work"
exec /usr/bin/open "$old" --args "$@"
"#;

#[cfg(target_os = "macos")]
fn app(archive: &Path, relaunch: &[&str], team: Option<&str>) -> Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let executable = std::env::current_exe()?;
    let unsupported = |reason: &str| Error::Unsupported(reason.to_owned());
    let bundle = crate::bundle::bundle_of(&executable)
        .ok_or_else(|| unsupported("the application does not run from a bundle"))?;
    // Opened from a disk image or a download, the system runs a copy it hides.
    if bundle
        .components()
        .any(|part| part.as_os_str() == "AppTranslocation")
    {
        return Err(Error::NotWritable(bundle));
    }
    let (Some(parent), Some(name)) = (bundle.parent(), bundle.file_name()) else {
        return Err(unsupported("the application has no folder"));
    };
    let name = name.to_string_lossy();
    // Beside the bundle, so that the swap is two renames on one volume.
    let work = parent.join(format!(".{name}.update"));
    let backup = parent.join(format!(".{name}.previous"));
    let new = crate::bundle::unpack(archive, &work).map_err(|error| match error {
        Error::Io(denied) if denied.kind() == std::io::ErrorKind::PermissionDenied => {
            Error::NotWritable(bundle.clone())
        }
        other => other,
    })?;

    let checked = signed(&new, team.map(str::to_owned).or_else(|| team_of(&bundle)));
    if let Err(error) = checked {
        let _ = std::fs::remove_dir_all(&work);
        return Err(error);
    }
    let _ = std::fs::remove_dir_all(&backup);

    Command::new("/bin/sh")
        .args(["-c", SWAP, "roadcase-update"])
        .arg(std::process::id().to_string())
        .args([&bundle, &new, &backup, &work])
        .args(relaunch)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Its own group: what ends this process must not end the swap.
        .process_group(0)
        .spawn()?;
    Ok(())
}

/// The team that signed a bundle, if one did.
#[cfg(target_os = "macos")]
fn team_of(bundle: &Path) -> Option<String> {
    let output = std::process::Command::new("/usr/bin/codesign")
        .args(["--display", "--verbose=2"])
        .arg(bundle)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stderr);
    let team = text
        .lines()
        .find_map(|line| line.strip_prefix("TeamIdentifier="))?
        .trim();
    (team != "not set" && team.chars().all(|c| c.is_ascii_alphanumeric())).then(|| team.to_owned())
}

/// Checks a bundle's signature, nested code included, and who signed it.
#[cfg(target_os = "macos")]
fn signed(bundle: &Path, team: Option<String>) -> Result<()> {
    let mut command = std::process::Command::new("/usr/bin/codesign");
    command.args(["--verify", "--deep", "--strict"]);
    if let Some(team) = team {
        if !team.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(Error::CodeSignature(format!("{team} is not a team")));
        }
        command.arg(format!(
            "-R=anchor apple generic and certificate leaf[subject.OU] = \"{team}\""
        ));
    }
    let output = command.arg(bundle).output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Error::CodeSignature(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_installer_hears_how_to_start_the_application_again() {
        assert_eq!(nsis_arguments(&[]), "/S /UPDATE");
        assert_eq!(
            nsis_arguments(&["--background", "--profile=work"]),
            "/S /UPDATE /RELAUNCH=--background --profile=work"
        );
    }

    #[test]
    fn arguments_an_installer_could_misread_are_refused() {
        let path = Path::new("setup.exe");
        for argument in ["", "two words", "say\"this\"", "/DIR=C:\\"] {
            let release = Release {
                kind: Kind::Unknown,
                sha256: "",
                team: None,
                running: "1.0.0",
                artifact: "",
            };
            let refused = run(path, &release, &[argument]);
            assert!(
                matches!(&refused, Err(Error::Unsupported(reason)) if reason.contains("argument")),
                "{refused:?}"
            );
        }
    }
}
