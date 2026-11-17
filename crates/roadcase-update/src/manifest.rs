//! What a release publishes: the manifest and its signature.

use crate::{Error, Result};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The manifest's file name in a feed.
pub const MANIFEST: &str = "latest.json";
/// The signature's file name in a feed.
pub const SIGNATURE: &str = "latest.json.minisig";

/// A release: its version, and what each platform installs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The version, which an application compares with its own.
    pub version: Version,
    /// What changed, for a person to read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// The artifacts, by [`target`].
    pub artifacts: BTreeMap<String, Artifact>,
}

/// What one platform downloads and installs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    /// How it installs.
    pub kind: Kind,
    /// Where it is: absolute, or relative to the manifest.
    pub url: String,
    /// Its SHA-256, in lowercase hexadecimal.
    pub sha256: String,
    /// Its size in bytes.
    pub size: u64,
}

/// How an artifact installs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Kind {
    /// An NSIS installer that reads `/UPDATE` and `/RELAUNCH=`.
    Nsis,
    /// A gzipped tar archive of one signed `.app` bundle.
    AppTarGz,
    /// A Linux AppImage, which replaces the running one's file.
    #[serde(rename = "appimage")]
    AppImage,
    /// A Debian package, under a `<target>-deb` key such as
    /// `linux-x86_64-deb`. apt installs it from the application's apt
    /// repository: the feed only announces the version to copies installed
    /// from a `.deb`, which never install it themselves.
    Deb,
    /// An Arch Linux package, under a `<target>-pacman` key such as
    /// `linux-x86_64-pacman`. pacman installs it, from a pacman repository or
    /// the AUR: the feed only announces the version.
    Pacman,
    /// A kind a later version of this crate knows.
    #[serde(other)]
    Unknown,
}

/// The public half of the key releases are signed with.
#[derive(Debug, Clone)]
pub struct PublicKey(minisign_verify::PublicKey);

impl PublicKey {
    /// Reads a minisign public key: the base64 line, or the whole file.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] when it is neither.
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        let key = if text.contains('\n') {
            minisign_verify::PublicKey::decode(text)
        } else {
            minisign_verify::PublicKey::from_base64(text)
        };
        key.map(Self)
            .map_err(|error| Error::Config(format!("the public key does not read: {error}")))
    }
}

impl Manifest {
    /// Reads a manifest, once `signature` proves that the holder of `key`
    /// published exactly these bytes.
    ///
    /// # Errors
    ///
    /// [`Error::Signature`]; [`Error::Manifest`].
    pub fn verified(bytes: &[u8], signature: &str, key: &PublicKey) -> Result<Self> {
        let signature =
            minisign_verify::Signature::decode(signature).map_err(|_| Error::Signature)?;
        key.0
            .verify(bytes, &signature, false)
            .map_err(|_| Error::Signature)?;
        let manifest: Self =
            serde_json::from_slice(bytes).map_err(|error| Error::Manifest(error.to_string()))?;
        for (target, artifact) in &manifest.artifacts {
            let hex = artifact.sha256.len() == 64
                && artifact
                    .sha256
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c));
            if !hex {
                return Err(Error::Manifest(format!("{target} has no SHA-256")));
            }
        }
        Ok(manifest)
    }
}

/// The name of the platform this build runs on, as manifests key their
/// artifacts: `windows-x86_64`, `macos-aarch64`, `macos-x86_64`,
/// `linux-x86_64`, `linux-aarch64`.
pub fn target() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}
