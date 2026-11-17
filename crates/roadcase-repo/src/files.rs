//! Files and folders of a repository.

use crate::error::io;
use crate::{Error, Result};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Refuses a name that would leave its folder or is not plain.
pub(crate) fn check_name(what: &str, name: &str) -> Result<()> {
    let plain = !name.is_empty()
        && !name.starts_with(['.', '-'])
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"+-._@".contains(&c));
    if plain {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "{what} {name:?} is not a plain name: letters, digits and +-._@"
        )))
    }
}

pub(crate) fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A file's SHA-256, in lowercase hexadecimal, and its size.
pub(crate) fn digest(path: &Path) -> Result<(String, u64)> {
    let mut file = std::fs::File::open(path).map_err(io(path))?;
    let mut hash = Sha256::new();
    let size = std::io::copy(&mut file, &mut hash).map_err(io(path))?;
    Ok((hex(&hash.finalize()), size))
}

/// Writes a file, making its folder.
pub(crate) fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    std::fs::write(path, bytes).map_err(io(path))
}

/// gzip, with no name and no time, so the same bytes give the same file.
pub(crate) fn gzip(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = flate2::GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), flate2::Compression::best());
    encoder
        .write_all(bytes)
        .and_then(|()| encoder.finish())
        .map_err(io("gzip"))
}

/// Copies `from` to `to`, unless `to` is already the same file. A different
/// file at `to` is refused: a published package never changes.
pub(crate) fn place(from: &Path, to: &Path) -> Result<()> {
    if to.exists() {
        if digest(from)? == digest(to)? {
            return Ok(());
        }
        return Err(Error::Package(format!(
            "{} is already published with other contents: a published version never changes, \
             so raise the version or the package's release",
            to.display()
        )));
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    let partial = to.with_extension("partial");
    std::fs::copy(from, &partial).map_err(io(from))?;
    std::fs::rename(&partial, to).map_err(io(to))
}

/// The names of the folders in `dir`, sorted; none when it does not exist.
pub(crate) fn folders(dir: &Path) -> Result<Vec<String>> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(Vec::new());
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(io(dir))?;
        if entry.file_type().map_err(io(entry.path()))?.is_dir() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// Every file under `dir`, sorted.
pub(crate) fn walk(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(folder) = pending.pop() {
        for entry in std::fs::read_dir(&folder).map_err(io(&folder))? {
            let entry = entry.map_err(io(&folder))?;
            let kind = entry.file_type().map_err(io(entry.path()))?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                found.push(entry.path());
            }
        }
    }
    found.sort();
    Ok(found)
}
