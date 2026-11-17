use crate::PackageManager;
use std::path::PathBuf;

/// What went wrong with an update.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The file system's.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The feed could not be reached, or did not answer with the file.
    #[error("the update server could not be reached: {0}")]
    Transport(String),
    /// A URL is not HTTPS, or does not read.
    #[error("{0} is not a URL updates may come from")]
    Url(String),
    /// The public key, or the version this application says it is, does not read.
    #[error("the updater is not set up: {0}")]
    Config(String),
    /// The manifest is not signed by the key this application holds.
    #[error("the update's signature is not ours")]
    Signature,
    /// The manifest is signed, and does not read.
    #[error("the update's description does not read: {0}")]
    Manifest(String),
    /// What was downloaded is not what the manifest describes.
    #[error("the downloaded update is not the one that was published")]
    Mismatch,
    /// The update cannot be installed by this build on this system.
    #[error("this update cannot be installed here: {0}")]
    Unsupported(String),
    /// The application's folder cannot be replaced by this person.
    #[error("{} cannot be replaced: move the application to a folder you can change", .0.display())]
    NotWritable(PathBuf),
    /// An AppImage update was asked of an application that does not run from
    /// an AppImage (`$APPIMAGE` is not set, or names no file), such as one
    /// started from its build folder or installed by a package manager.
    #[error("the application does not run from an AppImage, so it cannot replace it")]
    NotAppImage,
    /// The application was installed by the system's package manager, which
    /// is what updates it: an application shows "update with your package
    /// manager" instead of downloading anything.
    #[error("the application is installed by {manager}: update it with {manager}")]
    ManagedBySystem {
        /// The package manager that installed it.
        manager: PackageManager,
    },
    /// The new application's code signature is missing, broken or someone else's.
    #[error("the update's code signature was refused: {0}")]
    CodeSignature(String),
    /// Signing went wrong.
    #[cfg(feature = "sign")]
    #[error("signing failed: {0}")]
    Sign(String),
}

/// Results of this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;
