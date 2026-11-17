//! Copying files into a bundle.

use crate::config::Source;
use crate::error::io;
use crate::{Context, Error, Result};
use std::path::{Path, PathBuf};

/// The first candidate of `source` that exists and is not empty.
pub(crate) fn resolve(context: &Context, source: &Source, what: &str) -> Result<PathBuf> {
    let candidates = match source {
        Source::One(path) => std::slice::from_ref(path),
        Source::Candidates(paths) => paths.as_slice(),
    };
    let mut tried = Vec::new();
    for candidate in candidates {
        let path = context.path(Path::new(candidate))?;
        if std::fs::metadata(&path).is_ok_and(|meta| meta.is_file() && meta.len() > 0) {
            return Ok(path);
        }
        tried.push(path.display().to_string());
    }
    Err(Error::Missing(format!(
        "{what}: no file, or only an empty one, at {}",
        tried.join(", ")
    )))
}

/// Copies a file, making its folder, and marks it executable when asked.
pub(crate) fn copy_file(from: &Path, to: &Path, executable: bool) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    std::fs::copy(from, to).map_err(io(from))?;
    if executable {
        mark_executable(to)?;
    }
    Ok(())
}

#[cfg(unix)]
fn mark_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(io(path))
}

#[cfg(not(unix))]
fn mark_executable(_: &Path) -> Result<()> {
    Ok(())
}

/// Copies a folder's contents into `to`, keeping symbolic links as links.
pub(crate) fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).map_err(io(to))?;
    for entry in std::fs::read_dir(from).map_err(io(from))? {
        let entry = entry.map_err(io(from))?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        let kind = entry.file_type().map_err(io(&source))?;
        if kind.is_symlink() {
            link(&source, &target)?;
        } else if kind.is_dir() {
            copy_dir(&source, &target)?;
        } else {
            std::fs::copy(&source, &target).map_err(io(&source))?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn link(source: &Path, target: &Path) -> Result<()> {
    let points_to = std::fs::read_link(source).map_err(io(source))?;
    std::os::unix::fs::symlink(points_to, target).map_err(io(target))
}

#[cfg(not(unix))]
fn link(source: &Path, target: &Path) -> Result<()> {
    std::fs::copy(source, target)
        .map_err(io(source))
        .map(|_| ())
}

/// Copies what a resource's `from` names (a file, a folder, or a glob) into
/// the folder `to`. Answers with what was placed.
pub(crate) fn copy_resource(context: &Context, from: &str, to: &Path) -> Result<Vec<PathBuf>> {
    let filled = context.fill(from)?;
    let pattern = context.base.join(&filled);
    let sources: Vec<PathBuf> = if filled.contains(['*', '?', '[']) {
        let pattern = pattern.to_string_lossy();
        glob::glob(&pattern)
            .map_err(|error| Error::Config(format!("resource {from}: {error}")))?
            .collect::<Result<_, _>>()
            .map_err(|error| Error::Missing(format!("resource {from}: {error}")))?
    } else {
        vec![pattern]
    };
    let sources: Vec<PathBuf> = sources.into_iter().filter(|path| path.exists()).collect();
    if sources.is_empty() {
        return Err(Error::Missing(format!(
            "resource {from}: nothing at {}",
            context.base.join(&filled).display()
        )));
    }
    std::fs::create_dir_all(to).map_err(io(to))?;
    let mut placed = Vec::new();
    for source in sources {
        let Some(name) = source.file_name() else {
            continue;
        };
        let target = to.join(name);
        if source.is_dir() {
            copy_dir(&source, &target)?;
        } else {
            std::fs::copy(&source, &target).map_err(io(&source))?;
        }
        placed.push(target);
    }
    Ok(placed)
}

/// Reads a plist file whose top is a dictionary.
pub(crate) fn read_dictionary(path: &Path) -> Result<plist::Dictionary> {
    let value = plist::Value::from_file(path)
        .map_err(|error| Error::Plist(format!("{}: {error}", path.display())))?;
    value
        .into_dictionary()
        .ok_or_else(|| Error::Plist(format!("{}: not a dictionary", path.display())))
}

/// Writes a dictionary as an XML plist.
pub(crate) fn write_dictionary(path: &Path, dictionary: plist::Dictionary) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    plist::Value::Dictionary(dictionary)
        .to_file_xml(path)
        .map_err(|error| Error::Plist(format!("{}: {error}", path.display())))
}

/// Writes a zip of `files`, each at the top under its own name.
pub(crate) fn zip(files: &[PathBuf], to: &Path) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    let file = std::fs::File::create(to).map_err(io(to))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let failed =
        |error: zip::result::ZipError| Error::Command(format!("{}: {error}", to.display()));
    for path in files {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        zip.start_file(name, options).map_err(failed)?;
        let bytes = std::fs::read(path).map_err(io(path))?;
        zip.write_all(&bytes).map_err(io(to))?;
    }
    zip.finish().map_err(failed)?;
    Ok(())
}
