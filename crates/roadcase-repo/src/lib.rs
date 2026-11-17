//! # roadcase-repo
//!
//! Package repositories as static folders, for the Linux packages roadcase
//! makes, signed in Rust with no `gpg` or agent:
//!
//! - [`apt`]: `pool/` and `dists/<suite>/` with `Packages`, `Release`,
//!   `Release.gpg` and `InRelease`;
//! - [`pacman`]: `<arch>/<repo>.db` and `<repo>.files`, the packages and
//!   every one's `.sig`;
//! - [`aur`]: a `-bin` `PKGBUILD` and `.SRCINFO` for the AUR, pointing at
//!   the packages of a pacman repository;
//! - [`key`]: the OpenPGP signing key, made and used with
//!   [rPGP](https://github.com/rpgp/rpgp).
//!
//! Each folder is uploaded as it is to any HTTPS host.

pub mod apt;
mod archive;
pub mod aur;
mod date;
pub mod deb;
mod error;
mod files;
pub mod key;
pub mod pacman;
pub mod version;

pub use error::{Error, Result};
