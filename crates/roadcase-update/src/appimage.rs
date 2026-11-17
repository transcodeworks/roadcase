//! A Linux AppImage replacing its own file, then starting again.
//!
//! An AppImage named after its version (`App-1.2.0-x86_64.AppImage`) is
//! replaced by the new release under the feed artifact's own name, beside
//! it, and the old file removed; one with a stable name (`App.AppImage`) is
//! replaced in place.
//!
//! Built on every Unix, so its tests run on macOS too; only Linux installs
//! with it.
#![cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]

use crate::{Error, Result};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Waits for the application to leave, then starts the AppImage again.
const RELAUNCH: &str = r#"
pid="$1"; image="$2"; shift 2
tries=0
while kill -0 "$pid" 2>/dev/null; do
  tries=$((tries + 1))
  [ "$tries" -gt 300 ] && exit 1
  sleep 0.2
done
exec "$image" "$@"
"#;

/// Variables the AppImage runtime sets for the running copy, which the next
/// one sets for itself.
const RUNTIME_ENV: &[&str] = &["APPIMAGE", "APPDIR", "ARGV0", "OWD"];

/// Replaces the AppImage at `image` (the running one's `$APPIMAGE`, of
/// version `running`) with the verified download `staged`, published as
/// `artifact`, then starts it again with `relaunch` once the process `pid`
/// has left.
pub(crate) fn install(
    staged: &Path,
    sha256: &str,
    image: Option<PathBuf>,
    running: &str,
    artifact: &str,
    relaunch: &[&str],
    pid: u32,
) -> Result<()> {
    let image = image
        .filter(|image| !image.as_os_str().is_empty() && image.is_file())
        .ok_or(Error::NotAppImage)?;
    let placed = place(staged, sha256, &image, running, artifact)?;
    relaunch_after(pid, &placed, relaunch)
}

/// Puts the new release where [`destination`] says, removes the old file
/// when the new one went beside it, and answers where the new one is.
pub(crate) fn place(
    staged: &Path,
    sha256: &str,
    image: &Path,
    running: &str,
    artifact: &str,
) -> Result<PathBuf> {
    let mut target = destination(image, running, artifact);
    // Another name for the running file (a link) is still the running file.
    if target != image && same_file(&target, image) {
        target = image.to_path_buf();
    }
    replace(staged, sha256, &target)?;
    if target != image {
        // Only once the new file is whole and in place; a running copy
        // keeps its open file, and one that cannot go stays.
        let _ = std::fs::remove_file(image);
    }
    Ok(target)
}

/// Where the new release goes: beside `image` under the artifact's name when
/// `image`'s name holds the running version and the artifact's name is a
/// safe AppImage name; otherwise `image` itself.
pub(crate) fn destination(image: &Path, running: &str, artifact: &str) -> PathBuf {
    let (Some(folder), Some(name)) = (image.parent(), image.file_name().and_then(|n| n.to_str()))
    else {
        return image.to_path_buf();
    };
    if names_version(name, running) && safe_name(artifact) {
        folder.join(artifact)
    } else {
        image.to_path_buf()
    }
}

/// Whether `name` holds `version` as a token of its own: not touching a
/// letter, a digit or a `.` on either side, so that `1.2.0` is not found in
/// `11.2.0` or `1.2.0.1`. A `v` just before it is part of the token
/// (`App-v1.2.0`), and the `.AppImage` ending does not count against it.
pub(crate) fn names_version(name: &str, version: &str) -> bool {
    if version.is_empty() {
        return false;
    }
    let stem = name.strip_suffix(".AppImage").unwrap_or(name);
    let apart = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric() && c != '.');
    (0..stem.len()).any(|at| {
        stem.is_char_boundary(at)
            && stem[at..].starts_with(version)
            && {
                let before = &stem[..at];
                apart(before.chars().next_back())
                    || before
                        .strip_suffix(['v', 'V'])
                        .is_some_and(|rest| apart(rest.chars().next_back()))
            }
            && apart(stem[at + version.len()..].chars().next())
    })
}

/// A file name, not a path, that is visible and an AppImage.
pub(crate) fn safe_name(name: &str) -> bool {
    name.len() > ".AppImage".len()
        && name.ends_with(".AppImage")
        && !name.starts_with('.')
        && !name
            .chars()
            .any(|c| c == '/' || c == '\\' || c.is_control())
}

/// Whether two paths name one file.
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// Copies `staged` into a file beside `image`, checks it is the release the
/// manifest describes and an AppImage, makes it executable, and renames it
/// to `image`: one atomic step, over any file of that name, which the running
/// copy survives, since it keeps its own file open.
pub(crate) fn replace(staged: &Path, sha256: &str, image: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let (Some(folder), Some(name)) = (image.parent(), image.file_name()) else {
        return Err(Error::NotAppImage);
    };
    let temporary = folder.join(format!(".{}.update", name.to_string_lossy()));
    let refused = |error: std::io::Error| {
        if matches!(
            error.kind(),
            std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
        ) {
            Error::NotWritable(image.to_path_buf())
        } else {
            Error::Io(error)
        }
    };
    let placed = (|| {
        let mut from = std::fs::File::open(staged)?;
        let mut to = std::fs::File::create(&temporary).map_err(refused)?;
        std::io::copy(&mut from, &mut to)?;
        to.sync_all()?;
        drop(to);
        let (found, _) = crate::updater::digest(&temporary)?;
        if found != sha256 {
            return Err(Error::Mismatch);
        }
        if !is_appimage(&temporary)? {
            return Err(Error::Unsupported("it is not an AppImage".into()));
        }
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&temporary, image).map_err(refused)?;
        // The rename itself survives a power cut once the folder is synced.
        if let Ok(folder) = std::fs::File::open(folder) {
            let _ = folder.sync_all();
        }
        Ok(())
    })();
    if placed.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    placed
}

/// An ELF file with the AppImage type 2 mark: `AI` and 2 at offset 8.
fn is_appimage(path: &Path) -> Result<bool> {
    let mut head = [0u8; 11];
    let mut file = std::fs::File::open(path)?;
    if file.read_exact(&mut head).is_err() {
        return Ok(false);
    }
    Ok(&head[..4] == b"\x7fELF" && &head[8..11] == b"AI\x02")
}

/// Starts a shell that waits for `pid` to leave, then runs `image` with
/// `relaunch`, in its own process group so that what ends this process does
/// not end it.
pub(crate) fn relaunch_after(pid: u32, image: &Path, relaunch: &[&str]) -> Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", RELAUNCH, "roadcase-update"])
        .arg(pid.to_string())
        .arg(image)
        .args(relaunch)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    for name in RUNTIME_ENV {
        command.env_remove(name);
    }
    command.spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;

    const NEW: &[u8] = b"\x7fELF\x02\x01\x01\0AI\x02 the new release";

    fn sha(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("staged.AppImage");
        std::fs::write(&staged, NEW).unwrap();
        let apps = dir.path().join("Applications");
        std::fs::create_dir(&apps).unwrap();
        let image = apps.join("Example-0.1.0-x86_64.AppImage");
        std::fs::write(&image, b"\x7fELF\x02\x01\x01\0AI\x02 the old release").unwrap();
        (dir, staged, image)
    }

    fn leftovers(folder: &Path) -> Vec<String> {
        std::fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with('.'))
            .collect()
    }

    #[test]
    fn the_new_release_takes_the_old_ones_place() {
        let (_dir, staged, image) = setup();
        replace(&staged, &sha(NEW), &image).unwrap();
        assert_eq!(std::fs::read(&image).unwrap(), NEW);
        let mode = std::fs::metadata(&image).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
        assert!(leftovers(image.parent().unwrap()).is_empty());
    }

    #[test]
    fn a_name_with_the_running_version_takes_the_new_releases_name() {
        let image = Path::new("/apps/App-1.2.0-x86_64.AppImage");
        let new = "App-1.3.0-x86_64.AppImage";
        assert_eq!(
            destination(image, "1.2.0", new),
            Path::new("/apps/App-1.3.0-x86_64.AppImage")
        );
        for name in [
            "App_1.2.0.AppImage",
            "1.2.0-App.AppImage",
            "App-1.2.0.AppImage",
            "App-v1.2.0-x86_64.AppImage",
            "v1.2.0.AppImage",
        ] {
            assert!(names_version(name, "1.2.0"), "{name}");
        }
        assert!(names_version(
            "App-1.2.0-beta.1-x86_64.AppImage",
            "1.2.0-beta.1"
        ));
    }

    #[test]
    fn a_stable_name_is_replaced_in_place() {
        for image in [
            "/apps/App-x86_64.AppImage",
            "/apps/App.AppImage",
            "/apps/1.2.AppImage",
        ] {
            let image = Path::new(image);
            assert_eq!(
                destination(image, "1.2.0", "App-1.3.0-x86_64.AppImage"),
                image
            );
        }
    }

    #[test]
    fn a_version_inside_another_is_not_the_running_one() {
        for name in [
            "App-11.2.0-x86_64.AppImage",
            "App-1.2.01-x86_64.AppImage",
            "App-1.2.0.1-x86_64.AppImage",
            "App-21.2.0.AppImage",
            "App-dev1.2.0.AppImage",
            "App1.2.0.AppImage",
            "Appv1.2.0.AppImage",
        ] {
            assert!(!names_version(name, "1.2.0"), "{name}");
            let image = Path::new("/apps").join(name);
            assert_eq!(destination(&image, "1.2.0", "App-1.3.0.AppImage"), image);
        }
        assert!(!names_version("App-1.2.0.AppImage", ""));
    }

    #[test]
    fn an_unsafe_artifact_name_falls_back_to_replacing_in_place() {
        let image = Path::new("/apps/App-1.2.0-x86_64.AppImage");
        for artifact in [
            "",
            ".AppImage",
            "../App-1.3.0.AppImage",
            "sub/App-1.3.0.AppImage",
            "sub\\App-1.3.0.AppImage",
            ".hidden-1.3.0.AppImage",
            "App-1.3.0.tar.gz",
            "App-1.3.0.appimage",
            "App-1.3.0\n.AppImage",
        ] {
            assert!(!safe_name(artifact), "{artifact:?}");
            assert_eq!(destination(image, "1.2.0", artifact), image, "{artifact:?}");
        }
    }

    #[test]
    fn a_versioned_appimage_is_replaced_by_the_new_name_and_the_old_removed() {
        let (_dir, staged, image) = setup();
        let folder = image.parent().unwrap();
        let placed = place(
            &staged,
            &sha(NEW),
            &image,
            "0.1.0",
            "Example-0.2.0-x86_64.AppImage",
        )
        .unwrap();
        assert_eq!(placed, folder.join("Example-0.2.0-x86_64.AppImage"));
        assert_eq!(std::fs::read(&placed).unwrap(), NEW);
        let mode = std::fs::metadata(&placed).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
        assert!(!image.exists(), "the old release was kept");
        assert!(leftovers(folder).is_empty());
    }

    #[test]
    fn a_file_already_there_under_the_new_name_is_overwritten() {
        let (_dir, staged, image) = setup();
        let folder = image.parent().unwrap();
        let there = folder.join("Example-0.2.0-x86_64.AppImage");
        std::fs::write(&there, b"an earlier, broken copy").unwrap();
        let placed = place(
            &staged,
            &sha(NEW),
            &image,
            "0.1.0",
            "Example-0.2.0-x86_64.AppImage",
        )
        .unwrap();
        assert_eq!(placed, there);
        assert_eq!(std::fs::read(&there).unwrap(), NEW);
        assert!(!image.exists());
    }

    #[test]
    fn a_stable_name_or_an_unsafe_artifact_keeps_its_file() {
        let (dir, staged, _) = setup();
        let stable = dir.path().join("Applications/Example-x86_64.AppImage");
        std::fs::write(&stable, b"old").unwrap();
        let placed = place(
            &staged,
            &sha(NEW),
            &stable,
            "0.1.0",
            "Example-0.2.0-x86_64.AppImage",
        )
        .unwrap();
        assert_eq!(placed, stable);
        assert_eq!(std::fs::read(&stable).unwrap(), NEW);
        assert!(
            !stable
                .parent()
                .unwrap()
                .join("Example-0.2.0-x86_64.AppImage")
                .exists()
        );

        let (_dir, staged, image) = setup();
        let placed = place(&staged, &sha(NEW), &image, "0.1.0", "../Example.AppImage").unwrap();
        assert_eq!(placed, image);
        assert_eq!(std::fs::read(&image).unwrap(), NEW);
    }

    #[test]
    fn a_link_to_the_running_file_under_the_new_name_is_the_running_file() {
        let (_dir, staged, image) = setup();
        let folder = image.parent().unwrap();
        std::os::unix::fs::symlink(&image, folder.join("Example-0.2.0-x86_64.AppImage")).unwrap();
        let placed = place(
            &staged,
            &sha(NEW),
            &image,
            "0.1.0",
            "Example-0.2.0-x86_64.AppImage",
        )
        .unwrap();
        assert_eq!(placed, image);
        assert_eq!(std::fs::read(&image).unwrap(), NEW);
    }

    #[test]
    fn a_refused_download_leaves_the_versioned_file_alone() {
        let (_dir, staged, image) = setup();
        let old = std::fs::read(&image).unwrap();
        let refused = place(
            &staged,
            &sha(b"other"),
            &image,
            "0.1.0",
            "Example-0.2.0-x86_64.AppImage",
        );
        assert!(matches!(refused, Err(Error::Mismatch)), "{refused:?}");
        assert_eq!(std::fs::read(&image).unwrap(), old);
        let folder = image.parent().unwrap();
        assert!(!folder.join("Example-0.2.0-x86_64.AppImage").exists());
        assert!(leftovers(folder).is_empty());
    }

    #[test]
    fn a_changed_download_or_another_file_is_refused_and_the_old_one_kept() {
        let (_dir, staged, image) = setup();
        let old = std::fs::read(&image).unwrap();
        let refused = replace(&staged, &sha(b"other"), &image);
        assert!(matches!(refused, Err(Error::Mismatch)), "{refused:?}");

        std::fs::write(&staged, b"#!/bin/sh\necho not an AppImage\n").unwrap();
        let refused = replace(&staged, &sha(b"#!/bin/sh\necho not an AppImage\n"), &image);
        assert!(matches!(refused, Err(Error::Unsupported(_))), "{refused:?}");

        assert_eq!(std::fs::read(&image).unwrap(), old);
        assert!(leftovers(image.parent().unwrap()).is_empty());
    }

    #[test]
    fn a_folder_this_person_cannot_change_is_not_writable() {
        let (_dir, staged, image) = setup();
        let folder = image.parent().unwrap();
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o555)).unwrap();
        // Root writes anywhere: then there is nothing to test.
        let probe = folder.join("probe");
        if std::fs::write(&probe, b"").is_ok() {
            let _ = std::fs::remove_file(&probe);
        } else {
            let refused = replace(&staged, &sha(NEW), &image);
            assert!(
                matches!(&refused, Err(Error::NotWritable(path)) if path == &image),
                "{refused:?}"
            );
        }
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn without_an_appimage_there_is_nothing_to_replace() {
        let (dir, staged, _) = setup();
        for image in [
            None,
            Some(PathBuf::new()),
            Some(dir.path().join("missing.AppImage")),
        ] {
            let refused = install(&staged, &sha(NEW), image, "0.1.0", "A.AppImage", &[], 1);
            assert!(matches!(refused, Err(Error::NotAppImage)), "{refused:?}");
        }
    }

    #[test]
    fn it_starts_again_once_the_application_has_left() {
        let dir = tempfile::tempdir().unwrap();
        let ran = dir.path().join("ran");
        let image = dir.path().join("Example.AppImage");
        std::fs::write(
            &image,
            format!("#!/bin/sh\necho \"$APPDIR|$*\" > '{}'\n", ran.display()),
        )
        .unwrap();
        std::fs::set_permissions(&image, std::fs::Permissions::from_mode(0o755)).unwrap();

        // The application: a process that leaves after a moment, reaped so
        // that it does not linger as a zombie.
        let mut application = std::process::Command::new("sleep")
            .arg("0.6")
            .spawn()
            .unwrap();
        let pid = application.id();
        let left = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let waiter = {
            let left = left.clone();
            std::thread::spawn(move || {
                application.wait().unwrap();
                left.store(true, std::sync::atomic::Ordering::SeqCst);
            })
        };

        relaunch_after(pid, &image, &["--background", "--profile=work"]).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !ran.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "the AppImage was not started again"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(left.load(std::sync::atomic::Ordering::SeqCst));
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(
            std::fs::read_to_string(&ran).unwrap().trim(),
            "|--background --profile=work"
        );
        waiter.join().unwrap();
    }
}
