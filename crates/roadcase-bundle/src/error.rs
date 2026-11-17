use std::path::PathBuf;

/// What went wrong while packaging.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A file could not be read or written.
    #[error("{}: {source}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// The config does not read, or asks for something that does not exist.
    #[error("{0}")]
    Config(String),
    /// The config reads, and has these problems.
    #[error("the config has {} problem(s):\n  - {}", .0.len(), .0.join("\n  - "))]
    Invalid(Vec<String>),
    /// A value or placeholder could not be resolved.
    #[error("{0}")]
    Value(String),
    /// A file the config names is not there.
    #[error("{0}")]
    Missing(String),
    /// A property list does not read or is not what it should be.
    #[error("{0}")]
    Plist(String),
    /// A command failed.
    #[error("{0}")]
    Command(String),
}

/// Results of this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Attaches a path to an I/O error.
pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(std::io::Error) -> Error {
    let path = path.into();
    move |source| Error::Io { path, source }
}
