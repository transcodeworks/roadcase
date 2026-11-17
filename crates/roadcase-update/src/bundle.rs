//! A macOS application bundle: finding the running one, and unpacking a new one.
//! Built on every platform, so its tests run wherever the crate is built.
#![cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]

use crate::{Error, Result};
use std::path::{Path, PathBuf};

/// The bundle an executable runs from: `Name.app/Contents/MacOS/name`.
pub(crate) fn bundle_of(executable: &Path) -> Option<PathBuf> {
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let named = |path: &Path, name: &str| path.file_name().is_some_and(|found| found == name);
    (named(macos, "MacOS")
        && named(contents, "Contents")
        && bundle
            .extension()
            .is_some_and(|extension| extension == "app"))
    .then(|| bundle.to_path_buf())
}

/// Unpacks a `.app.tar.gz` into the new folder `into`, and answers with the
/// one bundle it held.
pub(crate) fn unpack(archive: &Path, into: &Path) -> Result<PathBuf> {
    if into.exists() {
        std::fs::remove_dir_all(into)?;
    }
    std::fs::create_dir_all(into)?;
    let file = std::fs::File::open(archive)?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    archive.set_preserve_permissions(true);
    // Refuses entries that would land outside `into`.
    archive.unpack(into)?;
    let mut bundles = Vec::new();
    for entry in std::fs::read_dir(into)? {
        let path = entry?.path();
        if path.is_dir() && path.extension().is_some_and(|extension| extension == "app") {
            bundles.push(path);
        }
    }
    match bundles.len() {
        1 => Ok(bundles.remove(0)),
        found => Err(Error::Unsupported(format!(
            "the archive holds {found} applications, not one"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_executable_knows_its_bundle() {
        let inside = Path::new("/Applications/Example.app/Contents/MacOS/example-app");
        assert_eq!(
            bundle_of(inside),
            Some(PathBuf::from("/Applications/Example.app"))
        );
        assert_eq!(bundle_of(Path::new("/usr/local/bin/example-app")), None);
        assert_eq!(
            bundle_of(Path::new("/tmp/Example/Contents/MacOS/example-app")),
            None
        );
    }

    fn archive_of(dir: &Path, names: &[&str]) -> PathBuf {
        let path = dir.join("update.tar.gz");
        let gz = flate2::write::GzEncoder::new(
            std::fs::File::create(&path).unwrap(),
            flate2::Compression::fast(),
        );
        let mut tar = tar::Builder::new(gz);
        for name in names {
            let mut header = tar::Header::new_gnu();
            header.set_size(2);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append_data(&mut header, name, &b"hi"[..]).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
        path
    }

    #[test]
    fn an_archive_unpacks_to_its_one_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let archive = archive_of(dir.path(), &["Example.app/Contents/MacOS/example-app"]);
        let into = dir.path().join("new");
        std::fs::create_dir_all(into.join("left-over")).unwrap();

        let bundle = unpack(&archive, &into).unwrap();
        assert_eq!(bundle, into.join("Example.app"));
        assert!(bundle.join("Contents/MacOS/example-app").is_file());
        assert!(!into.join("left-over").exists());
    }

    #[test]
    fn an_archive_with_no_bundle_or_two_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        for names in [
            &["example-app"][..],
            &["A.app/Contents/x", "B.app/Contents/x"][..],
        ] {
            let archive = archive_of(dir.path(), names);
            let refused = unpack(&archive, &dir.path().join("new"));
            assert!(matches!(refused, Err(Error::Unsupported(_))), "{refused:?}");
        }
    }
}
