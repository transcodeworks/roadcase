//! Making keys and publishing releases. Release tooling only: an application
//! never holds the secret key.

use crate::manifest::{Artifact, Kind, MANIFEST, Manifest, SIGNATURE};
use crate::{Error, Result};
use semver::Version;
use std::path::{Path, PathBuf};

/// A new key pair, as text.
#[derive(Debug, Clone)]
pub struct Keys {
    /// The public key: one base64 line, compiled into the application.
    pub public: String,
    /// The secret key, in minisign's file format: with no password from
    /// [`generate`], for a secret store; encrypted with one from
    /// [`generate_with_password`], for a file.
    pub secret: String,
}

/// The environment variable [`secret_from_env`] reads the secret key from.
pub const SECRET_KEY_ENV: &str = "ROADCASE_UPDATE_SECRET_KEY";
/// The name the secret key had before this crate was renamed, still read
/// when [`SECRET_KEY_ENV`] is not set.
pub const LEGACY_SECRET_KEY_ENV: &str = "CC_UPDATE_SECRET_KEY";

/// The secret key from the environment: `ROADCASE_UPDATE_SECRET_KEY`, or
/// `CC_UPDATE_SECRET_KEY` when that is not set or empty.
pub fn secret_from_env() -> Option<String> {
    secret_from(|name| std::env::var(name).ok())
}

/// The environment variable a secret key's password is read from.
pub const PASSWORD_ENV: &str = "ROADCASE_UPDATE_KEY_PASSWORD";

/// The password for an encrypted secret key from `ROADCASE_UPDATE_KEY_PASSWORD`,
/// when it is set and not empty.
pub fn password_from_env() -> Option<String> {
    std::env::var(PASSWORD_ENV)
        .ok()
        .filter(|value| !value.is_empty())
}

fn secret_from(lookup: impl Fn(&str) -> Option<String>) -> Option<String> {
    [SECRET_KEY_ENV, LEGACY_SECRET_KEY_ENV]
        .into_iter()
        .find_map(|name| lookup(name).filter(|value| !value.is_empty()))
}

fn failed(error: impl std::fmt::Display) -> Error {
    Error::Sign(error.to_string())
}

/// Makes a key pair.
///
/// # Errors
///
/// [`Error::Sign`] when the system gives no randomness.
pub fn generate() -> Result<Keys> {
    let pair = minisign::KeyPair::generate_unencrypted_keypair().map_err(failed)?;
    Ok(Keys {
        public: pair.pk.to_base64(),
        secret: pair.sk.to_box(None).map_err(failed)?.into_string(),
    })
}

/// Makes a key pair whose secret key is encrypted with `password`, in
/// minisign's format: the scrypt KDF (`Sc`) with a random salt and the
/// parameters minisign's Rust implementation uses, so `minisign` and `rsign`
/// read it with the same password.
///
/// # Errors
///
/// [`Error::Sign`] when the password is empty or the system gives no
/// randomness.
pub fn generate_with_password(password: &str) -> Result<Keys> {
    if password.is_empty() {
        return Err(failed("the password is empty"));
    }
    let pair =
        minisign::KeyPair::generate_encrypted_keypair(Some(password.to_owned())).map_err(failed)?;
    Ok(Keys {
        public: pair.pk.to_base64(),
        secret: pair
            .sk
            .to_box(Some("minisign encrypted secret key"))
            .map_err(failed)?
            .into_string(),
    })
}

/// Whether a secret key is encrypted, so that reading it needs a password.
///
/// # Errors
///
/// [`Error::Sign`] when the text is not a minisign secret key, or is
/// encrypted with a KDF other than minisign's scrypt.
pub fn has_password(secret: &str) -> Result<bool> {
    // The key line starts with the signature algorithm, `Ed`, then the KDF:
    // two zero bytes for none, `Sc` for scrypt. Eight base64 characters hold
    // the first six bytes.
    let line = secret
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("untrusted comment:"))
        .ok_or_else(|| failed("the secret key is empty"))?;
    if line.len() < 8 {
        return Err(failed("the secret key is too short"));
    }
    let mut bits = 0u64;
    for byte in line.bytes().take(8) {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err(failed("the secret key is not base64")),
        };
        bits = bits << 6 | u64::from(value);
    }
    let head = bits.to_be_bytes();
    match [head[4], head[5]] {
        [0, 0] => Ok(false),
        [b'S', b'c'] => Ok(true),
        _ => Err(failed("the secret key is encrypted with an unknown KDF")),
    }
}

/// Reads a secret key, with its password when it is encrypted. A password
/// given for a key without one is ignored.
fn read_key(secret: &str, password: Option<&str>) -> Result<minisign::SecretKey> {
    let encrypted = has_password(secret)?;
    let sk = minisign::SecretKeyBox::from_string(secret).map_err(failed)?;
    if !encrypted {
        return sk.into_unencrypted_secret_key().map_err(failed);
    }
    let password = password.ok_or_else(|| {
        failed(format!(
            "the secret key is encrypted, and no password was given (set {PASSWORD_ENV})"
        ))
    })?;
    // minisign checks the decrypted key against its checksum, so a wrong
    // password never yields a key.
    sk.into_secret_key(Some(password.to_owned()))
        .map_err(|error| match error.kind() {
            minisign::ErrorKind::Verify => failed("wrong password for the secret key"),
            _ => failed(error),
        })
}

/// Signs a manifest's bytes with a secret key that has no password, and
/// answers with the signature file's text.
///
/// # Errors
///
/// [`Error::Sign`] when the key does not read, or is encrypted.
pub fn sign(manifest: &[u8], secret: &str) -> Result<String> {
    sign_with_password(manifest, secret, None)
}

/// Signs a manifest's bytes, decrypting the secret key with `password` when
/// it is encrypted, and answers with the signature file's text.
///
/// # Errors
///
/// [`Error::Sign`] when the key does not read, is encrypted and no password
/// is given, or the password is wrong.
pub fn sign_with_password(manifest: &[u8], secret: &str, password: Option<&str>) -> Result<String> {
    let key = read_key(secret, password)?;
    let signature =
        minisign::sign(None, &key, std::io::Cursor::new(manifest), None, None).map_err(failed)?;
    Ok(signature.into_string())
}

/// One platform's part of a release.
#[derive(Debug, Clone)]
pub struct Part {
    /// The platform, as [`crate::target`] names it.
    pub target: String,
    /// How the file installs.
    pub kind: Kind,
    /// The file.
    pub path: PathBuf,
    /// Where the file is published, when that is not the feed folder: an
    /// absolute URL, such as a package's place in an apt or pacman
    /// repository. The file is then measured and not copied. `None` copies
    /// it into the feed folder, under its own name.
    pub url: Option<String>,
}

/// Writes a release into `folder`, ready to upload as it is: the artifacts,
/// `latest.json` and its signature. A part with a [`Part::url`] is published
/// elsewhere (a package in its repository): the manifest names that URL with
/// the file's size and SHA-256, and the file is not copied.
///
/// A manifest already there for the same version keeps its other platforms,
/// so that each platform's build machine can add its own part.
///
/// The secret key has no password; [`publish_with_password`] takes either.
///
/// # Errors
///
/// [`Error::Sign`]; the file system's.
pub fn publish(
    folder: &Path,
    version: &Version,
    notes: Option<&str>,
    parts: &[Part],
    secret: &str,
) -> Result<Manifest> {
    publish_with_password(folder, version, notes, parts, secret, None)
}

/// [`publish`], decrypting the secret key with `password` when it is
/// encrypted. The key is read before anything is written, so a wrong password
/// leaves the folder as it was.
///
/// # Errors
///
/// [`Error::Sign`], also when the key is encrypted and the password is
/// missing or wrong; the file system's.
pub fn publish_with_password(
    folder: &Path,
    version: &Version,
    notes: Option<&str>,
    parts: &[Part],
    secret: &str,
    password: Option<&str>,
) -> Result<Manifest> {
    let key = read_key(secret, password)?;
    for part in parts {
        check_target(part)?;
        if let Some(url) = &part.url
            && !(url.starts_with("https://") || url.starts_with("http://"))
        {
            return Err(failed(format!(
                "{}: {url} is not an absolute http(s) URL",
                part.target
            )));
        }
    }
    std::fs::create_dir_all(folder)?;
    let location = folder.join(MANIFEST);
    let before = std::fs::read(&location)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Manifest>(&bytes).ok())
        .filter(|manifest| &manifest.version == version);
    let mut manifest = before.unwrap_or_else(|| Manifest {
        version: version.clone(),
        notes: None,
        artifacts: Default::default(),
    });
    if let Some(notes) = notes {
        manifest.notes = Some(notes.to_owned());
    }
    for part in parts {
        let (url, measured) = match &part.url {
            Some(url) => (url.clone(), part.path.clone()),
            None => {
                let name = part
                    .path
                    .file_name()
                    .ok_or_else(|| failed(format!("{} is not a file", part.path.display())))?;
                let placed = folder.join(name);
                if !placed.exists() || !same_file(&placed, &part.path) {
                    std::fs::copy(&part.path, &placed)?;
                }
                (encode(&name.to_string_lossy()), placed)
            }
        };
        let (sha256, size) = crate::updater::digest(&measured)?;
        manifest.artifacts.insert(
            part.target.clone(),
            Artifact {
                kind: part.kind,
                url,
                sha256,
                size,
            },
        );
    }
    let bytes = serde_json::to_vec_pretty(&manifest).map_err(failed)?;
    let signature = minisign::sign(None, &key, std::io::Cursor::new(&bytes), None, None)
        .map_err(failed)?
        .into_string();
    std::fs::write(&location, &bytes)?;
    std::fs::write(folder.join(SIGNATURE), signature)?;
    Ok(manifest)
}

/// A system package is announced under `<target>-deb` or `<target>-pacman`,
/// and nothing else is: an installed copy looks for its own kind under its
/// own key.
fn check_target(part: &Part) -> Result<()> {
    let suffix = match part.kind {
        Kind::Deb => Some("-deb"),
        Kind::Pacman => Some("-pacman"),
        _ => None,
    };
    let fine = match suffix {
        Some(suffix) => part.target.ends_with(suffix) && part.target.len() > suffix.len(),
        None => !part.target.ends_with("-deb") && !part.target.ends_with("-pacman"),
    };
    if fine {
        Ok(())
    } else {
        Err(failed(format!(
            "{}: a deb is published under <target>-deb (linux-x86_64-deb), a pacman package \
             under <target>-pacman, and nothing else under those",
            part.target
        )))
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// A file name as a relative URL.
fn encode(name: &str) -> String {
    let mut encoded = String::new();
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// The file an artifact's relative URL names, as [`publish`] wrote it.
pub fn file_name(artifact: &Artifact) -> String {
    let last = artifact.url.rsplit('/').next().unwrap_or_default();
    let mut bytes = Vec::new();
    let mut rest = last.as_bytes();
    while let Some((&byte, after)) = rest.split_first() {
        let escaped = (byte == b'%' && after.len() >= 2)
            .then(|| std::str::from_utf8(&after[..2]).ok())
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(decoded) => {
                bytes.push(decoded);
                rest = &after[2..];
            }
            None => {
                bytes.push(byte);
                rest = after;
            }
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// A file's SHA-256, in lowercase hexadecimal, and its size.
///
/// # Errors
///
/// The file system's.
pub fn digest(path: &Path) -> Result<(String, u64)> {
    Ok(crate::updater::digest(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PublicKey;

    #[test]
    fn the_secret_key_is_read_under_its_new_name_then_its_old_one() {
        let environment = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(found, _)| *found == name)
                    .map(|(_, value)| (*value).to_owned())
            }
        };
        let both = environment(&[
            ("ROADCASE_UPDATE_SECRET_KEY", "new"),
            ("CC_UPDATE_SECRET_KEY", "old"),
        ]);
        assert_eq!(secret_from(both).as_deref(), Some("new"));
        let old = environment(&[
            ("ROADCASE_UPDATE_SECRET_KEY", ""),
            ("CC_UPDATE_SECRET_KEY", "old"),
        ]);
        assert_eq!(secret_from(old).as_deref(), Some("old"));
        assert_eq!(secret_from(environment(&[])), None);
    }

    #[test]
    fn an_encrypted_key_signs_with_its_password_and_only_with_it() {
        let keys = generate_with_password("correct horse").unwrap();
        assert!(has_password(&keys.secret).unwrap());
        assert!(!has_password(&generate().unwrap().secret).unwrap());
        assert!(keys.secret.lines().nth(1).unwrap().starts_with("RWRTY0Iy"));

        let manifest = br#"{"version":"0.1.0","artifacts":{}}"#;
        let signature = sign_with_password(manifest, &keys.secret, Some("correct horse")).unwrap();
        let public = PublicKey::parse(&keys.public).unwrap();
        Manifest::verified(manifest, &signature, &public).unwrap();

        let wrong = sign_with_password(manifest, &keys.secret, Some("wrong horse")).unwrap_err();
        assert!(wrong.to_string().contains("wrong password"), "{wrong}");
        let missing = sign(manifest, &keys.secret).unwrap_err();
        assert!(missing.to_string().contains(PASSWORD_ENV), "{missing}");
        assert!(generate_with_password("").is_err());

        // Nothing is written with a wrong password.
        let dir = tempfile::tempdir().unwrap();
        let feed = dir.path().join("feed");
        let refused = publish_with_password(
            &feed,
            &Version::new(0, 1, 0),
            None,
            &[],
            &keys.secret,
            Some("wrong horse"),
        );
        assert!(refused.is_err());
        assert!(!feed.exists());
        publish_with_password(
            &feed,
            &Version::new(0, 1, 0),
            None,
            &[],
            &keys.secret,
            Some("correct horse"),
        )
        .unwrap();
        Manifest::verified(
            &std::fs::read(feed.join(MANIFEST)).unwrap(),
            &std::fs::read_to_string(feed.join(SIGNATURE)).unwrap(),
            &public,
        )
        .unwrap();
    }

    #[test]
    fn a_password_for_a_key_without_one_is_ignored() {
        let keys = generate().unwrap();
        let manifest = b"{}";
        let signature = sign_with_password(manifest, &keys.secret, Some("unused")).unwrap();
        let public = PublicKey::parse(&keys.public).unwrap();
        Manifest::verified(manifest, &signature, &public).unwrap_err(); // not a manifest
        assert!(signature.starts_with("untrusted comment: "));
        assert!(has_password("not a key").is_err());
    }

    #[test]
    fn packages_are_published_under_their_own_keys() {
        let keys = generate().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let deb = dir.path().join("app_0.2.0-1_amd64.deb");
        std::fs::write(&deb, b"deb").unwrap();
        let feed = dir.path().join("feed");
        let version = Version::new(0, 2, 0);
        let part = |target: &str, kind| Part {
            target: target.into(),
            kind,
            path: deb.clone(),
            url: None,
        };
        for wrong in [
            part("linux-x86_64", Kind::Deb),
            part("-deb", Kind::Deb),
            part("linux-x86_64-deb", Kind::Pacman),
            part("linux-x86_64-deb", Kind::AppImage),
        ] {
            assert!(publish(&feed, &version, None, &[wrong], &keys.secret).is_err());
        }
        assert!(!feed.exists());
        let manifest = publish(
            &feed,
            &version,
            None,
            &[part("linux-x86_64-deb", Kind::Deb)],
            &keys.secret,
        )
        .unwrap();
        assert_eq!(manifest.artifacts["linux-x86_64-deb"].kind, Kind::Deb);
    }

    #[test]
    fn a_package_published_elsewhere_is_named_by_its_url_and_not_copied() {
        let keys = generate().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let deb = dir.path().join("app_0.2.0-1_amd64.deb");
        std::fs::write(&deb, b"deb").unwrap();
        let feed = dir.path().join("feed");
        let version = Version::new(0, 2, 0);
        let part = |url: &str| Part {
            target: "linux-x86_64-deb".into(),
            kind: Kind::Deb,
            path: deb.clone(),
            url: Some(url.into()),
        };
        let relative = part("pool/main/a/app/app_0.2.0-1_amd64.deb");
        assert!(publish(&feed, &version, None, &[relative], &keys.secret).is_err());
        let url = "https://packages.example.com/apt/pool/main/a/app/app_0.2.0-1_amd64.deb";
        let manifest = publish(&feed, &version, None, &[part(url)], &keys.secret).unwrap();
        let artifact = &manifest.artifacts["linux-x86_64-deb"];
        assert_eq!(artifact.url, url);
        assert_eq!(artifact.size, 3);
        assert_eq!(artifact.sha256, digest(&deb).unwrap().0);
        assert!(!feed.join("app_0.2.0-1_amd64.deb").exists());
        assert!(feed.join(MANIFEST).is_file());
    }

    #[test]
    fn a_published_release_verifies_and_keeps_other_platforms() {
        let keys = generate().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let built = dir.path().join("built");
        std::fs::create_dir(&built).unwrap();
        let installer = built.join("Example 0.2.0 setup.exe");
        let archive = built.join("Example.app.tar.gz");
        std::fs::write(&installer, b"installer").unwrap();
        std::fs::write(&archive, b"archive").unwrap();
        let feed = dir.path().join("feed");
        let version = Version::new(0, 2, 0);

        let part = |target: &str, kind, path: &Path| Part {
            target: target.into(),
            kind,
            path: path.to_path_buf(),
            url: None,
        };
        let windows = part("windows-x86_64", Kind::Nsis, &installer);
        publish(&feed, &version, Some("Faster."), &[windows], &keys.secret).unwrap();
        let mac = part("macos-aarch64", Kind::AppTarGz, &archive);
        publish(&feed, &version, None, &[mac], &keys.secret).unwrap();

        let manifest = Manifest::verified(
            &std::fs::read(feed.join(MANIFEST)).unwrap(),
            &std::fs::read_to_string(feed.join(SIGNATURE)).unwrap(),
            &PublicKey::parse(&keys.public).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest.notes.as_deref(), Some("Faster."));
        assert_eq!(manifest.artifacts.len(), 2);
        let windows = &manifest.artifacts["windows-x86_64"];
        assert_eq!(windows.url, "Example%200.2.0%20setup.exe");
        assert_eq!(windows.size, 9);
        assert_eq!(file_name(windows), "Example 0.2.0 setup.exe");
        assert!(feed.join("Example 0.2.0 setup.exe").is_file());

        // The next version starts again.
        let next = Version::new(0, 3, 0);
        let mac = part("macos-aarch64", Kind::AppTarGz, &archive);
        let manifest = publish(&feed, &next, None, &[mac], &keys.secret).unwrap();
        assert_eq!(manifest.artifacts.len(), 1);
        assert_eq!(manifest.notes, None);
    }
}
