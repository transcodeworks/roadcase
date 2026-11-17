//! The repository signing key: OpenPGP, made and used in Rust, so that no
//! `gpg` or `gpg-agent` is needed where repositories are built.
//!
//! The key is one Ed25519 primary key (OpenPGP v4, EdDSA), which certifies
//! itself and signs. apt (`gpgv` and `sqv`) and pacman (GnuPG through gpgme)
//! accept it. Signatures are made over SHA-512.

use crate::{Error, Result};
use pgp::composed::{
    ArmorOptions, CleartextSignedMessage, Deserializable, DetachedSignature, KeyType,
    SecretKeyParamsBuilder, SignedPublicKey, SignedSecretKey,
};
use pgp::crypto::hash::HashAlgorithm;
use pgp::crypto::sym::SymmetricKeyAlgorithm;
use pgp::ser::Serialize;
use pgp::types::{KeyDetails, Password};
use smallvec::smallvec;

/// The environment variable [`SigningKey::from_env`] reads the armored secret
/// key from.
pub const KEY_ENV: &str = "ROADCASE_REPO_SIGNING_KEY";
/// The environment variable holding the secret key's password, when it has one.
pub const PASSWORD_ENV: &str = "ROADCASE_REPO_SIGNING_KEY_PASSWORD";

const HASH: HashAlgorithm = HashAlgorithm::Sha512;

fn key_error(error: impl std::fmt::Display) -> Error {
    Error::Key(error.to_string())
}

/// A new key pair.
#[derive(Debug, Clone)]
pub struct KeyPair {
    /// The secret key, armored: for a secret store.
    pub secret: String,
    /// The public key, armored (`.asc`): what people import.
    pub public: String,
    /// The public key as binary OpenPGP (`.gpg`): a keyring file for apt's
    /// `Signed-By`.
    pub public_binary: Vec<u8>,
    /// The key's fingerprint, in upper-case hexadecimal.
    pub fingerprint: String,
}

/// Makes a signing key for `user_id`, such as
/// `Example Packages <packages@example.com>`, its secret part encrypted with
/// `password` when one is given.
///
/// # Errors
///
/// [`Error::Key`] when the user id is empty or generation fails.
pub fn generate(user_id: &str, password: Option<&str>) -> Result<KeyPair> {
    if user_id.trim().is_empty() {
        return Err(key_error(
            "the key needs a name, such as Example Packages <packages@example.com>",
        ));
    }
    let params = SecretKeyParamsBuilder::default()
        .key_type(KeyType::Ed25519Legacy)
        .can_certify(true)
        .can_sign(true)
        .primary_user_id(user_id.to_owned())
        .preferred_hash_algorithms(smallvec![HashAlgorithm::Sha512, HashAlgorithm::Sha256])
        .preferred_symmetric_algorithms(smallvec![
            SymmetricKeyAlgorithm::AES256,
            SymmetricKeyAlgorithm::AES128,
        ])
        .passphrase(
            password
                .filter(|password| !password.is_empty())
                .map(str::to_owned),
        )
        .build()
        .map_err(key_error)?;
    let secret = params.generate(rand::thread_rng()).map_err(key_error)?;
    let public = PublicKey(secret.to_public_key());
    Ok(KeyPair {
        secret: secret
            .to_armored_string(ArmorOptions::default())
            .map_err(key_error)?,
        public: public.armored()?,
        public_binary: public.binary()?,
        fingerprint: public.fingerprint(),
    })
}

/// A secret key, ready to sign.
pub struct SigningKey {
    secret: SignedSecretKey,
    password: Password,
}

impl std::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SigningKey")
            .field("fingerprint", &self.fingerprint())
            .finish_non_exhaustive()
    }
}

impl SigningKey {
    /// Reads an armored secret key, with its password when it has one, and
    /// checks that it signs: a wrong password fails here, before anything is
    /// written.
    ///
    /// # Errors
    ///
    /// [`Error::Key`].
    pub fn parse(armored: &str, password: Option<&str>) -> Result<Self> {
        let (secret, _) = SignedSecretKey::from_armor_single(armored.as_bytes())
            .map_err(|error| key_error(format!("the signing key does not read: {error}")))?;
        let key = Self {
            secret,
            password: password.map(Password::from).unwrap_or_else(Password::empty),
        };
        key.detached(b"").map_err(|error| {
            key_error(format!(
                "the signing key does not sign (is its password {PASSWORD_ENV} right?): {error}"
            ))
        })?;
        Ok(key)
    }

    /// The key from `ROADCASE_REPO_SIGNING_KEY`, and its password from
    /// `ROADCASE_REPO_SIGNING_KEY_PASSWORD` (or the variable `password_env`
    /// names).
    ///
    /// # Errors
    ///
    /// [`Error::Key`] when the variable is not set, or as [`SigningKey::parse`].
    pub fn from_env(password_env: Option<&str>) -> Result<Self> {
        let armored = std::env::var(KEY_ENV)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| key_error(format!("{KEY_ENV} is not set")))?;
        let name = password_env.unwrap_or(PASSWORD_ENV);
        let password = std::env::var(name).ok().filter(|value| !value.is_empty());
        Self::parse(&armored, password.as_deref())
    }

    /// The public half.
    pub fn public(&self) -> PublicKey {
        PublicKey(self.secret.to_public_key())
    }

    /// The fingerprint, in upper-case hexadecimal.
    pub fn fingerprint(&self) -> String {
        format!("{:X}", self.secret.fingerprint())
    }

    fn signature(&self, data: &[u8]) -> Result<DetachedSignature> {
        DetachedSignature::sign_binary_data(
            rand::thread_rng(),
            &self.secret.primary_key,
            &self.password,
            HASH,
            data,
        )
        .map_err(key_error)
    }

    /// A detached signature of `data`, as binary OpenPGP: pacman's `.sig`.
    ///
    /// # Errors
    ///
    /// [`Error::Key`].
    pub fn detached(&self, data: &[u8]) -> Result<Vec<u8>> {
        self.signature(data)?.to_bytes().map_err(key_error)
    }

    /// A detached signature of `data`, armored: apt's `Release.gpg`.
    ///
    /// # Errors
    ///
    /// [`Error::Key`].
    pub fn detached_armored(&self, data: &[u8]) -> Result<String> {
        self.signature(data)?
            .to_armored_string(ArmorOptions::default())
            .map_err(key_error)
    }

    /// `text` with a cleartext signature: apt's `InRelease`.
    ///
    /// # Errors
    ///
    /// [`Error::Key`].
    pub fn clearsign(&self, text: &str) -> Result<String> {
        CleartextSignedMessage::new_many(text, |normalized| {
            let signature = DetachedSignature::sign_text_data(
                rand::thread_rng(),
                &self.secret.primary_key,
                &self.password,
                HASH,
                normalized.as_bytes(),
            )?;
            Ok(vec![signature.signature])
        })
        .and_then(|message| message.to_armored_string(ArmorOptions::default()))
        .map_err(key_error)
    }
}

/// A public key: what verifies a repository.
#[derive(Debug, Clone)]
pub struct PublicKey(SignedPublicKey);

impl PublicKey {
    /// Reads a public key, armored or binary.
    ///
    /// # Errors
    ///
    /// [`Error::Key`].
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let key = if bytes.trim_ascii_start().starts_with(b"-----BEGIN") {
            SignedPublicKey::from_armor_single(bytes).map(|(key, _)| key)
        } else {
            SignedPublicKey::from_bytes(bytes)
        };
        key.map(Self)
            .map_err(|error| key_error(format!("the public key does not read: {error}")))
    }

    /// Armored, for people to import.
    ///
    /// # Errors
    ///
    /// [`Error::Key`].
    pub fn armored(&self) -> Result<String> {
        self.0
            .to_armored_string(ArmorOptions::default())
            .map_err(key_error)
    }

    /// Binary, for a keyring file.
    ///
    /// # Errors
    ///
    /// [`Error::Key`].
    pub fn binary(&self) -> Result<Vec<u8>> {
        self.0.to_bytes().map_err(key_error)
    }

    /// The fingerprint, in upper-case hexadecimal.
    pub fn fingerprint(&self) -> String {
        format!("{:X}", self.0.fingerprint())
    }

    /// Checks a detached signature of `data`, binary or armored.
    ///
    /// # Errors
    ///
    /// [`Error::Key`] when it does not read or does not match.
    pub fn verify_detached(&self, data: &[u8], signature: &[u8]) -> Result<()> {
        let signature = if signature.trim_ascii_start().starts_with(b"-----BEGIN") {
            DetachedSignature::from_armor_single(signature).map(|(signature, _)| signature)
        } else {
            DetachedSignature::from_bytes(signature)
        }
        .map_err(key_error)?;
        signature.verify(&self.0, data).map_err(key_error)
    }

    /// Checks a cleartext-signed message, and answers with the text it signs,
    /// with `\n` line ends.
    ///
    /// # Errors
    ///
    /// [`Error::Key`] when it does not read or does not match.
    pub fn verify_cleartext(&self, message: &str) -> Result<String> {
        let (message, _) = CleartextSignedMessage::from_string(message).map_err(key_error)?;
        message.verify(&self.0).map_err(key_error)?;
        Ok(message.signed_text().replace("\r\n", "\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_key_signs_and_its_public_half_verifies() {
        let pair = generate("Example Packages <packages@example.com>", None).unwrap();
        assert!(
            pair.secret
                .starts_with("-----BEGIN PGP PRIVATE KEY BLOCK-----")
        );
        assert!(
            pair.public
                .starts_with("-----BEGIN PGP PUBLIC KEY BLOCK-----")
        );
        assert_eq!(pair.fingerprint.len(), 40);

        let key = SigningKey::parse(&pair.secret, None).unwrap();
        assert_eq!(key.fingerprint(), pair.fingerprint);
        let public = PublicKey::parse(pair.public.as_bytes()).unwrap();
        let binary = PublicKey::parse(&pair.public_binary).unwrap();
        assert_eq!(binary.fingerprint(), pair.fingerprint);

        let data = b"Origin: Example\n";
        let signature = key.detached(data).unwrap();
        public.verify_detached(data, &signature).unwrap();
        assert!(
            public
                .verify_detached(b"Origin: Other\n", &signature)
                .is_err()
        );
        let armored = key.detached_armored(data).unwrap();
        assert!(armored.starts_with("-----BEGIN PGP SIGNATURE-----"));
        binary.verify_detached(data, armored.as_bytes()).unwrap();

        let text = "Origin: Example\nSuite: stable\n";
        let signed = key.clearsign(text).unwrap();
        assert!(signed.starts_with("-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA512\n"));
        assert_eq!(
            public.verify_cleartext(&signed).unwrap().trim_end(),
            text.trim_end()
        );
        let changed = signed.replace("stable", "stolen");
        assert!(public.verify_cleartext(&changed).is_err());

        let other = generate("Someone Else <else@example.com>", None).unwrap();
        let other = PublicKey::parse(other.public.as_bytes()).unwrap();
        assert!(other.verify_detached(data, &signature).is_err());
    }

    #[test]
    fn a_key_with_a_password_signs_only_with_it() {
        let pair = generate(
            "Example Packages <packages@example.com>",
            Some("correct horse"),
        )
        .unwrap();
        assert!(SigningKey::parse(&pair.secret, Some("wrong horse")).is_err());
        assert!(SigningKey::parse(&pair.secret, None).is_err());
        let key = SigningKey::parse(&pair.secret, Some("correct horse")).unwrap();
        let public = PublicKey::parse(pair.public.as_bytes()).unwrap();
        public
            .verify_detached(b"data", &key.detached(b"data").unwrap())
            .unwrap();
        assert!(generate(" ", None).is_err());
    }
}
