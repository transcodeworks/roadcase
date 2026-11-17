//! The key, signature and manifest formats are fixed: copies already installed
//! hold public keys in them, and release tooling outside this crate makes and
//! reads the same keys. These fixtures come from the crate's earlier name,
//! `cc-update`, at the revision installed copies use:
//!
//! - `FIXED` was assembled outside Rust from a seed of 32 0x01 bytes and the
//!   key id 01..08, and signed and verified with by `cc-update`;
//! - `GENERATED` was made by `cc-update`'s own `generate`;
//! - `fixtures/feed` was written by `cc-update`'s `publish` with `FIXED`, from
//!   the two files in `fixtures/built`.
//!
//! All of them are test keys and sign nothing real.

use roadcase_update::{Kind, MANIFEST, Manifest, PublicKey, SIGNATURE, Version};
use std::path::{Path, PathBuf};

const FIXED_PUBLIC: &str = "RWQBAgMEBQYHCIqI4910CfGV/VLbLTy6XXLKZwm/HZQSG/N0iAG0D29c";
#[cfg(feature = "sign")]
const FIXED_SECRET: &str = "untrusted comment: rsign encrypted secret key\nRWQAAEIyAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAQIDBAUGBwgBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAYqI4910CfGV/VLbLTy6XXLKZwm/HZQSG/N0iAG0D29cbUZDefysPEmWq/vZYIit19Um7M1wio5/ihM3VITYF6Y=\n";

const GENERATED_PUBLIC: &str = "RWQ+fPOZE+vOYnIe42lEYwRu+HoT16EkqZZiI3e/cU9uWmuIxPMBC52/";
#[cfg(feature = "sign")]
const GENERATED_SECRET: &str = "untrusted comment: rsign encrypted secret key\nRWQAAEIyAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAPnzzmRPrzmIZA8MdIogRu+V8qHnhpHS5UdI7+d3XRYxQQBPbWGaOiXIe42lEYwRu+HoT16EkqZZiI3e/cU9uWmuIxPMBC52/sqMOGBuxKwaCOypi+FL5aVzKcwoxK4eQuDyjQKygSdM=\n";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn feed_of(folder: &Path) -> (Vec<u8>, String) {
    (
        std::fs::read(folder.join(MANIFEST)).unwrap(),
        std::fs::read_to_string(folder.join(SIGNATURE)).unwrap(),
    )
}

#[test]
fn a_feed_the_earlier_tool_signed_verifies() {
    let (bytes, signature) = feed_of(&fixtures().join("feed"));
    let key = PublicKey::parse(FIXED_PUBLIC).unwrap();
    let manifest = Manifest::verified(&bytes, &signature, &key).unwrap();
    assert_eq!(manifest.version, Version::new(1, 2, 0));
    assert_eq!(manifest.notes.as_deref(), Some("What changed."));
    assert_eq!(manifest.artifacts["macos-aarch64"].kind, Kind::AppTarGz);
    assert_eq!(manifest.artifacts["windows-x86_64"].kind, Kind::Nsis);
    assert_eq!(
        manifest.artifacts["windows-x86_64"].url,
        "Example%201.2.0%20setup.exe"
    );

    // And not with a key of another pair.
    let other = PublicKey::parse(GENERATED_PUBLIC).unwrap();
    assert!(Manifest::verified(&bytes, &signature, &other).is_err());
}

#[test]
fn a_public_key_file_with_its_comment_reads() {
    let file = format!("untrusted comment: minisign public key\n{FIXED_PUBLIC}\n");
    let (bytes, signature) = feed_of(&fixtures().join("feed"));
    let key = PublicKey::parse(&file).unwrap();
    assert!(Manifest::verified(&bytes, &signature, &key).is_ok());
}

#[cfg(feature = "sign")]
mod signing {
    use super::*;
    use roadcase_update::sign::{Part, generate, publish, sign};

    #[test]
    fn both_fixture_secret_keys_sign_for_their_public_keys() {
        for (secret, public) in [
            (FIXED_SECRET, FIXED_PUBLIC),
            (GENERATED_SECRET, GENERATED_PUBLIC),
        ] {
            let manifest = br#"{"version":"0.1.0","artifacts":{}}"#;
            let signature = sign(manifest, secret).unwrap();
            assert!(signature.starts_with("untrusted comment: "));
            let key = PublicKey::parse(public).unwrap();
            Manifest::verified(manifest, &signature, &key).unwrap();
        }
    }

    #[test]
    fn publishing_writes_the_same_manifest_bytes_as_before() {
        let dir = tempfile::tempdir().unwrap();
        let built = fixtures().join("built");
        let version = Version::new(1, 2, 0);
        let windows = Part {
            target: "windows-x86_64".into(),
            kind: Kind::Nsis,
            path: built.join("Example 1.2.0 setup.exe"),
            url: None,
        };
        let mac = Part {
            target: "macos-aarch64".into(),
            kind: Kind::AppTarGz,
            path: built.join("Example-1.2.0-aarch64.app.tar.gz"),
            url: None,
        };
        publish(
            dir.path(),
            &version,
            Some("What changed."),
            &[windows],
            FIXED_SECRET,
        )
        .unwrap();
        publish(dir.path(), &version, None, &[mac], FIXED_SECRET).unwrap();

        let (ours, signature) = feed_of(dir.path());
        let (theirs, _) = feed_of(&fixtures().join("feed"));
        assert_eq!(
            String::from_utf8(ours.clone()).unwrap(),
            String::from_utf8(theirs).unwrap()
        );
        let key = PublicKey::parse(FIXED_PUBLIC).unwrap();
        Manifest::verified(&ours, &signature, &key).unwrap();
    }

    #[test]
    fn a_new_pair_has_the_same_shape_as_the_fixtures() {
        let keys = generate().unwrap();
        assert_eq!(keys.public.len(), FIXED_PUBLIC.len());
        assert!(keys.public.starts_with("RW"));
        let mut lines = keys.secret.lines();
        assert_eq!(lines.next(), FIXED_SECRET.lines().next());
        assert_eq!(
            lines.next().map(str::len),
            FIXED_SECRET.lines().nth(1).map(str::len)
        );
        // The unencrypted marker: "Ed", then no KDF.
        assert!(keys.secret.lines().nth(1).unwrap().starts_with("RWQAAEIy"));
    }
}

/// What a release of this crate from before AppImages knew: an older
/// installed copy reads a manifest with Linux artifacts, and finds no kind it
/// could install in them.
mod older_copies {
    use super::*;
    use serde::Deserialize;
    use std::collections::BTreeMap;

    #[derive(Debug, PartialEq, Eq, Deserialize)]
    #[serde(rename_all = "kebab-case")]
    enum OldKind {
        Nsis,
        AppTarGz,
        #[serde(other)]
        Unknown,
    }

    #[derive(Debug, Deserialize)]
    struct OldArtifact {
        kind: OldKind,
    }

    #[derive(Debug, Deserialize)]
    struct OldManifest {
        artifacts: BTreeMap<String, OldArtifact>,
    }

    const WITH_LINUX: &str = r#"{
  "version": "1.3.0",
  "artifacts": {
    "linux-aarch64": { "kind": "appimage", "url": "Example-1.3.0-aarch64.AppImage", "sha256": "9e691d5adacf0e102f203100842ffaba4e2a7dfe3dbd536827d3c0db8c7e88f1", "size": 10 },
    "linux-x86_64": { "kind": "appimage", "url": "Example-1.3.0-x86_64.AppImage", "sha256": "9e691d5adacf0e102f203100842ffaba4e2a7dfe3dbd536827d3c0db8c7e88f1", "size": 10 },
    "macos-aarch64": { "kind": "app-tar-gz", "url": "Example-1.3.0-aarch64.app.tar.gz", "sha256": "9e691d5adacf0e102f203100842ffaba4e2a7dfe3dbd536827d3c0db8c7e88f1", "size": 10 },
    "windows-x86_64": { "kind": "flatpak-or-whatever-comes-next", "url": "x", "sha256": "9e691d5adacf0e102f203100842ffaba4e2a7dfe3dbd536827d3c0db8c7e88f1", "size": 10 }
  }
}"#;

    #[test]
    fn an_older_copy_ignores_the_appimage_kind() {
        let old: OldManifest = serde_json::from_str(WITH_LINUX).unwrap();
        assert_eq!(old.artifacts["linux-x86_64"].kind, OldKind::Unknown);
        assert_eq!(old.artifacts["macos-aarch64"].kind, OldKind::AppTarGz);
    }

    #[test]
    fn this_copy_reads_appimages_and_ignores_kinds_after_them() {
        let manifest: Manifest = serde_json::from_str(WITH_LINUX).unwrap();
        assert_eq!(manifest.artifacts["linux-x86_64"].kind, Kind::AppImage);
        assert_eq!(manifest.artifacts["linux-aarch64"].kind, Kind::AppImage);
        assert_eq!(manifest.artifacts["windows-x86_64"].kind, Kind::Unknown);
        // And writes the kind back the same way.
        let written = serde_json::to_string(&manifest.artifacts["linux-x86_64"]).unwrap();
        assert!(written.contains(r#""kind":"appimage""#), "{written}");
        let again: Manifest =
            serde_json::from_str(&serde_json::to_string(&manifest).unwrap()).unwrap();
        assert_eq!(again, manifest);
    }

    #[test]
    fn the_fixture_feed_still_reads_unchanged() {
        let (bytes, _) = feed_of(&fixtures().join("feed"));
        let old: OldManifest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(old.artifacts["windows-x86_64"].kind, OldKind::Nsis);
    }
}

/// Encrypted secret keys, in minisign's own format, both ways. In
/// `fixtures/encrypted`:
///
/// - `minisign.key` and `minisign.pub` were made by the reference `minisign`
///   tool (C, `minisign -G`) with the password `correct horse`, with its own
///   scrypt parameters;
/// - `roadcase.key` and `roadcase.pub` were made by `roadcase keygen
///   --password` with the password `battery staple`, and `latest.json.minisig`
///   is the reference tool's signature of `latest.json` with that key.
///
/// All of them are test keys and sign nothing real.
mod encrypted {
    use super::*;

    fn read(name: &str) -> String {
        std::fs::read_to_string(fixtures().join("encrypted").join(name)).unwrap()
    }

    #[test]
    fn the_reference_tool_signs_with_a_key_made_here() {
        let key = PublicKey::parse(&read("roadcase.pub")).unwrap();
        let manifest = Manifest::verified(
            read("latest.json").as_bytes(),
            &read("latest.json.minisig"),
            &key,
        )
        .unwrap();
        assert_eq!(manifest.version, Version::new(0, 1, 0));
    }

    #[cfg(feature = "sign")]
    #[test]
    fn keys_made_by_the_reference_tool_and_here_sign_with_their_passwords() {
        use roadcase_update::sign::{has_password, sign, sign_with_password};

        let manifest = br#"{"version":"0.1.0","artifacts":{}}"#;
        for (secret, public, password) in [
            ("minisign.key", "minisign.pub", "correct horse"),
            ("roadcase.key", "roadcase.pub", "battery staple"),
        ] {
            let secret = read(secret);
            assert!(has_password(&secret).unwrap());
            let signature = sign_with_password(manifest, &secret, Some(password)).unwrap();
            let key = PublicKey::parse(&read(public)).unwrap();
            Manifest::verified(manifest, &signature, &key).unwrap();

            let wrong = sign_with_password(manifest, &secret, Some("wrong")).unwrap_err();
            assert!(wrong.to_string().contains("wrong password"), "{wrong}");
            assert!(sign(manifest, &secret).is_err());
        }
    }

    #[cfg(feature = "sign")]
    #[test]
    fn keys_without_a_password_still_read_as_before() {
        use roadcase_update::sign::has_password;
        assert!(!has_password(FIXED_SECRET).unwrap());
        assert!(!has_password(GENERATED_SECRET).unwrap());
    }
}
