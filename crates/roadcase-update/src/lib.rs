//! # roadcase-update
//!
//! Signed self-updates for desktop applications, with no UI and no UI toolkit.
//!
//! A release is a folder of static files on any HTTPS host:
//!
//! - `latest.json`, the [`Manifest`]: the version, and for each platform an
//!   [`Artifact`] with its size and SHA-256;
//! - `latest.json.minisig`, a [minisign](https://jedisct1.github.io/minisign/)
//!   signature of the manifest;
//! - the artifacts: an NSIS installer for Windows, a `.app.tar.gz`
//!   for macOS, and an AppImage for Linux.
//!
//! The application holds the public key. The one signature covers the version
//! and every artifact's hash, so a host that is taken over can neither change
//! an artifact nor offer an older release.
//!
//! ```no_run
//! # #[cfg(feature = "http")]
//! # fn main() -> Result<(), roadcase_update::Error> {
//! use roadcase_update::{Config, Updater};
//!
//! let updater = Updater::new(Config::new(
//!     "https://updates.example.com/app/stable/",
//!     "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3",
//!     env!("CARGO_PKG_VERSION"),
//!     std::env::temp_dir().join("app-updates"),
//! )?)?;
//! if let Some(update) = updater.check()? {
//!     let staged = updater.download(&update, |_received, _total| {})?;
//!     staged.install(&[])?;
//!     // The installer waits for this process: leave now.
//!     std::process::exit(0);
//! }
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "http"))]
//! # fn main() {}
//! ```
//!
//! Every call blocks, so an application calls them from a thread of its own.
//!
//! ## Installing
//!
//! - **Windows** ([`Kind::Nsis`]): the installer runs silently with
//!   `/S /UPDATE /RELAUNCH=<arguments>`, replaces the application's files and
//!   starts it again. The application's script must read the last two, and
//!   wait for the application to leave; the README shows how.
//! - **macOS** ([`Kind::AppTarGz`]): the archive holds the signed, notarized
//!   `.app`. It is unpacked beside the running bundle, its code signature is
//!   checked, and a small shell script swaps the two once this process has
//!   left, then opens the new one.
//! - **Linux** ([`Kind::AppImage`]): the AppImage is copied beside the running
//!   one (`$APPIMAGE`), checked again against the manifest's SHA-256 and made
//!   executable. When the running one's name holds its version, the new one
//!   takes the artifact's name from the feed and the old file is removed;
//!   otherwise it is renamed over the running one. A small shell script
//!   starts it again once this process has left. An AppImage registers its
//!   desktop entry, URL schemes and document types itself, with [`desktop`].
//!
//! - **Linux packages** ([`Kind::Deb`], [`Kind::Pacman`]): a copy installed
//!   from a `.deb` or a pacman package is the system package manager's to
//!   update. The packages hold a marker file, [`INSTALL_MARKER`], that
//!   [`package_manager`] finds; [`Config::new`] reads it, so that
//!   [`Updater::check`] looks for the release under `<target>-deb` or
//!   `<target>-pacman` and [`Updater::download`] answers
//!   [`Error::ManagedBySystem`], for the application to say "update with your
//!   package manager".
//!
//! Releases are made with the `roadcase` command-line tool (`roadcase keygen`,
//! `roadcase publish`, `roadcase verify`), which the `sign` module backs.

#[cfg(unix)]
mod appimage;
mod bundle;
pub mod desktop;
mod error;
mod install;
mod manifest;
#[cfg(feature = "sign")]
pub mod sign;
mod system;
mod transport;
mod updater;

pub use error::{Error, Result};
pub use install::{NSIS_RELAUNCH, NSIS_SILENT, NSIS_UPDATE};
pub use manifest::{Artifact, Kind, MANIFEST, Manifest, PublicKey, SIGNATURE, target};
pub use semver::Version;
pub use system::{INSTALL_MARKER, PackageManager, package_manager, package_manager_of};
#[cfg(feature = "http")]
pub use transport::Http;
pub use transport::{Transport, TransportError};
pub use updater::{Config, Staged, Update, Updater};
