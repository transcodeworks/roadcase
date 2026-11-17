use std::path::PathBuf;

/// What went wrong with a repository.
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
    /// A package does not read, or is not what a repository holds.
    #[error("{0}")]
    Package(String),
    /// The signing key does not read or does not sign, or a signature does
    /// not verify.
    #[error("{0}")]
    Key(String),
    /// What was asked does not make a repository.
    #[error("{0}")]
    Invalid(String),
}

/// Results of this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Attaches a path to an I/O error.
pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(std::io::Error) -> Error {
    let path = path.into();
    move |source| Error::Io { path, source }
}
