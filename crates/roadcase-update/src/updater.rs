//! Checking a feed, and downloading what it offers.

use crate::manifest::{Artifact, Kind, MANIFEST, Manifest, PublicKey, SIGNATURE};
use crate::transport::{Transport, allowed};
use crate::{Error, PackageManager, Result, install};
use semver::Version;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use url::Url;

/// The most a manifest may weigh.
const MANIFEST_LIMIT: u64 = 1024 * 1024;
/// The most a signature may weigh.
const SIGNATURE_LIMIT: u64 = 16 * 1024;

/// What an application tells its updater.
#[derive(Debug, Clone)]
pub struct Config {
    /// The folder that holds `latest.json`.
    pub feed: Url,
    /// The key releases are signed with.
    pub key: PublicKey,
    /// The version that is running.
    pub current: Version,
    /// A folder of this application's own, where downloads wait. Everything
    /// else in it is removed.
    pub staging: PathBuf,
    /// The platform to look for in manifests: [`crate::target`] unless changed.
    pub target: String,
    /// The Apple team the new bundle must be signed by. When `None`, it must
    /// be the team that signed the running one, if any did.
    pub macos_team: Option<String>,
    /// The package manager that installed this copy, which
    /// [`crate::package_manager`] finds: releases are then looked for under
    /// `<target>-deb` or `<target>-pacman`, and announced only.
    pub package: Option<PackageManager>,
}

impl Config {
    /// A configuration for this platform.
    ///
    /// # Errors
    ///
    /// [`Error::Url`] when `feed` is not HTTPS; [`Error::Config`] when the key
    /// or the version does not read.
    pub fn new(
        feed: &str,
        public_key: &str,
        current: &str,
        staging: impl Into<PathBuf>,
    ) -> Result<Self> {
        let mut feed = Url::parse(feed).map_err(|_| Error::Url(feed.to_owned()))?;
        if !allowed(&feed) {
            return Err(Error::Url(feed.to_string()));
        }
        if !feed.path().ends_with('/') {
            let path = format!("{}/", feed.path());
            feed.set_path(&path);
        }
        let current = Version::parse(current)
            .map_err(|error| Error::Config(format!("version {current}: {error}")))?;
        Ok(Self {
            feed,
            key: PublicKey::parse(public_key)?,
            current,
            staging: staging.into(),
            target: crate::target(),
            macos_team: None,
            package: crate::package_manager(),
        })
    }
}

/// A newer release, checked against the key and not downloaded yet.
#[derive(Debug, Clone)]
pub struct Update {
    /// Its version.
    pub version: Version,
    /// What changed, for a person to read.
    pub notes: Option<String>,
    artifact: Artifact,
    url: Url,
}

impl Update {
    /// How many bytes the download is.
    pub fn size(&self) -> u64 {
        self.artifact.size
    }

    /// The package manager that installs this release, when the application
    /// was installed by one: it is then announced only, and the application
    /// says "update with your package manager" instead of downloading it.
    pub fn managed_by(&self) -> Option<PackageManager> {
        match self.artifact.kind {
            Kind::Deb => Some(PackageManager::Apt),
            Kind::Pacman => Some(PackageManager::Pacman),
            _ => None,
        }
    }
}

/// A downloaded release, checked against the manifest and ready to install.
#[derive(Debug, Clone)]
pub struct Staged {
    /// Its version.
    pub version: Version,
    path: PathBuf,
    kind: Kind,
    sha256: String,
    macos_team: Option<String>,
    /// The version that is running, which an AppImage's name may hold.
    running: Version,
    /// The artifact's name in the feed, as published.
    artifact: String,
}

impl Staged {
    /// Where the download is.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Starts the installation, which finishes after this process has left
    /// and then starts the application again with `relaunch` as its arguments.
    /// The caller exits as soon as this returns.
    ///
    /// # Errors
    ///
    /// [`Error::ManagedBySystem`] for a system package;
    /// [`Error::Unsupported`] when this build cannot install the artifact or
    /// an argument has characters other than letters, digits and `-_=.:`;
    /// [`Error::NotWritable`] and [`Error::CodeSignature`] on macOS;
    /// [`Error::NotAppImage`], [`Error::NotWritable`] and [`Error::Mismatch`]
    /// on Linux; the file system's.
    pub fn install(&self, relaunch: &[&str]) -> Result<()> {
        let running = self.running.to_string();
        let release = install::Release {
            kind: self.kind,
            sha256: &self.sha256,
            team: self.macos_team.as_deref(),
            running: &running,
            artifact: &self.artifact,
        };
        install::run(&self.path, &release, relaunch)
    }
}

/// Finds, downloads and installs newer releases of one application.
pub struct Updater {
    config: Config,
    transport: Box<dyn Transport>,
}

impl std::fmt::Debug for Updater {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Updater")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl Updater {
    /// An updater that fetches over HTTPS.
    ///
    /// # Errors
    ///
    /// [`Error::Transport`] when the TLS stack does not start.
    #[cfg(feature = "http")]
    pub fn new(config: Config) -> Result<Self> {
        let agent = format!("roadcase-update/{} ({})", config.current, config.target);
        let http = crate::transport::Http::new(&agent)
            .map_err(|error| Error::Transport(error.to_string()))?;
        Ok(Self::with_transport(config, http))
    }

    /// An updater that fetches with the application's own client.
    pub fn with_transport(config: Config, transport: impl Transport + 'static) -> Self {
        Self {
            config,
            transport: Box::new(transport),
        }
    }

    /// Asks the feed for a release newer than the running one, with an
    /// artifact for this platform: under [`Config::target`], or, for a copy a
    /// package manager installed, under `<target>-deb` or `<target>-pacman`,
    /// which [`Update::managed_by`] then names.
    ///
    /// # Errors
    ///
    /// [`Error::Transport`]; [`Error::Signature`]; [`Error::Manifest`];
    /// [`Error::Url`] when the artifact is somewhere updates may not come from.
    pub fn check(&self) -> Result<Option<Update>> {
        let join = |name: &str| {
            self.config
                .feed
                .join(name)
                .map_err(|_| Error::Url(name.to_owned()))
        };
        let location = join(MANIFEST)?;
        let bytes = self.fetch(&location, MANIFEST_LIMIT)?;
        let signature = self.fetch(&join(SIGNATURE)?, SIGNATURE_LIMIT)?;
        let signature = String::from_utf8(signature).map_err(|_| Error::Signature)?;
        let manifest = Manifest::verified(&bytes, &signature, &self.config.key)?;
        if manifest.version <= self.config.current {
            return Ok(None);
        }
        let key = match self.config.package {
            Some(manager) => manager.target(&self.config.target),
            None => self.config.target.clone(),
        };
        let Some(artifact) = manifest.artifacts.get(&key) else {
            return Ok(None);
        };
        let expected = match self.config.package {
            Some(manager) => artifact.kind == manager.kind(),
            None => !matches!(artifact.kind, Kind::Unknown | Kind::Deb | Kind::Pacman),
        };
        if !expected {
            return Ok(None);
        }
        let url = location
            .join(&artifact.url)
            .map_err(|_| Error::Url(artifact.url.clone()))?;
        if !allowed(&url) {
            return Err(Error::Url(url.to_string()));
        }
        Ok(Some(Update {
            version: manifest.version,
            notes: manifest.notes,
            artifact: artifact.clone(),
            url,
        }))
    }

    /// Downloads a release into the staging folder, or finds it there from an
    /// earlier run. `progress` hears the bytes received and the total.
    ///
    /// # Errors
    ///
    /// [`Error::ManagedBySystem`] for a release a package manager installs;
    /// [`Error::Transport`]; [`Error::Mismatch`] when what arrived is not what
    /// the manifest describes; the file system's.
    pub fn download(&self, update: &Update, mut progress: impl FnMut(u64, u64)) -> Result<Staged> {
        if let Some(manager) = update.managed_by() {
            return Err(Error::ManagedBySystem { manager });
        }
        let folder = self.config.staging.join(update.version.to_string());
        if let Ok(entries) = std::fs::read_dir(&self.config.staging) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path != folder {
                    let _ = std::fs::remove_dir_all(&path).or_else(|_| std::fs::remove_file(&path));
                }
            }
        }
        std::fs::create_dir_all(&folder)?;
        let name = file_name(&update.url, update.artifact.kind);
        let path = folder.join(&name);
        let staged = Staged {
            version: update.version.clone(),
            path: path.clone(),
            kind: update.artifact.kind,
            sha256: update.artifact.sha256.clone(),
            macos_team: self.config.macos_team.clone(),
            running: self.config.current.clone(),
            artifact: published_name(&update.url),
        };
        if holds(&path, &update.artifact) {
            progress(update.artifact.size, update.artifact.size);
            return Ok(staged);
        }

        let partial = folder.join(format!("{name}.partial"));
        let mut sink = Checked {
            file: std::fs::File::create(&partial)?,
            hash: Sha256::new(),
            left: update.artifact.size,
        };
        let total = update.artifact.size;
        let fetched = self
            .transport
            .get(&update.url, &mut sink, &mut |received, _| {
                progress(received, total);
            });
        let complete = sink.left == 0 && hex(&sink.hash.finalize()) == update.artifact.sha256;
        let synced = sink.file.sync_all();
        drop(sink.file);
        let outcome = match fetched {
            Err(error) => Err(Error::Transport(error.to_string())),
            Ok(()) if !complete => Err(Error::Mismatch),
            Ok(()) => synced
                .and_then(|()| std::fs::rename(&partial, &path))
                .map_err(Error::from),
        };
        if outcome.is_err() {
            let _ = std::fs::remove_file(&partial);
        }
        outcome.map(|()| staged)
    }

    fn fetch(&self, url: &Url, limit: u64) -> Result<Vec<u8>> {
        let mut sink = Capped {
            bytes: Vec::new(),
            limit,
        };
        self.transport
            .get(url, &mut sink, &mut |_, _| {})
            .map_err(|error| Error::Transport(error.to_string()))?;
        Ok(sink.bytes)
    }
}

/// Keeps a small file in memory, and refuses a large one.
struct Capped {
    bytes: Vec<u8>,
    limit: u64,
}

impl Write for Capped {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if (self.bytes.len() + buffer.len()) as u64 > self.limit {
            return Err(std::io::Error::other("the file is larger than it may be"));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Writes a download, hashing it, and refuses more than was published.
struct Checked {
    file: std::fs::File,
    hash: Sha256,
    left: u64,
}

impl Write for Checked {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let Some(left) = self.left.checked_sub(buffer.len() as u64) else {
            return Err(std::io::Error::other(
                "the download is larger than was published",
            ));
        };
        self.file.write_all(buffer)?;
        self.hash.update(buffer);
        self.left = left;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The SHA-256 of a file, in lowercase hexadecimal, and its size.
pub(crate) fn digest(path: &Path) -> std::io::Result<(String, u64)> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let size = std::io::copy(&mut file, &mut hash)?;
    Ok((hex(&hash.finalize()), size))
}

/// Whether the file at `path` is the artifact.
fn holds(path: &Path, artifact: &Artifact) -> bool {
    digest(path).is_ok_and(|(sha256, size)| size == artifact.size && sha256 == artifact.sha256)
}

/// The artifact's name as published: the last segment of its URL, decoded.
/// It is not safe as it is; what installs it checks it.
fn published_name(url: &Url) -> String {
    let last = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .unwrap_or_default();
    percent_encoding::percent_decode_str(last)
        .decode_utf8_lossy()
        .into_owned()
}

/// A safe name for the download, which Windows must be able to run.
fn file_name(url: &Url, kind: Kind) -> String {
    let mut name: String = published_name(url)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.trim_matches('.').is_empty() {
        name = "update".to_owned();
    }
    if kind == Kind::Nsis && !name.to_ascii_lowercase().ends_with(".exe") {
        name.push_str(".exe");
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::TransportError;
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A feed held in memory.
    #[derive(Default)]
    struct Feed {
        files: Mutex<BTreeMap<String, Vec<u8>>>,
        gets: AtomicUsize,
    }

    impl Feed {
        fn put(&self, name: &str, bytes: &[u8]) {
            self.files
                .lock()
                .unwrap()
                .insert(name.to_owned(), bytes.to_vec());
        }
    }

    impl Transport for std::sync::Arc<Feed> {
        fn get(
            &self,
            url: &Url,
            sink: &mut dyn Write,
            progress: &mut dyn FnMut(u64, Option<u64>),
        ) -> Result<(), TransportError> {
            self.gets.fetch_add(1, Ordering::Relaxed);
            let name = percent_encoding::percent_decode_str(url.path())
                .decode_utf8_lossy()
                .trim_start_matches("/app/")
                .to_owned();
            let bytes = self
                .files
                .lock()
                .unwrap()
                .get(&name)
                .cloned()
                .ok_or("404")?;
            for chunk in bytes.chunks(3) {
                sink.write_all(chunk)?;
            }
            progress(bytes.len() as u64, Some(bytes.len() as u64));
            Ok(())
        }
    }

    struct Fixture {
        feed: std::sync::Arc<Feed>,
        keys: minisign::KeyPair,
        dir: tempfile::TempDir,
    }

    const INSTALLER: &[u8] = b"an installer, as far as anyone can tell";

    impl Fixture {
        fn new() -> Self {
            Self {
                feed: std::sync::Arc::default(),
                keys: minisign::KeyPair::generate_unencrypted_keypair().unwrap(),
                dir: tempfile::tempdir().unwrap(),
            }
        }

        fn publish(&self, version: &str, target: &str, url: &str, body: &[u8]) {
            self.feed.put(url.rsplit('/').next().unwrap(), body);
            let manifest = Manifest {
                version: Version::parse(version).unwrap(),
                notes: Some("Faster.".into()),
                artifacts: BTreeMap::from([(
                    target.to_owned(),
                    Artifact {
                        kind: Kind::Nsis,
                        url: url.to_owned(),
                        sha256: hex(&Sha256::digest(body)),
                        size: body.len() as u64,
                    },
                )]),
            };
            self.sign(&serde_json::to_vec(&manifest).unwrap(), &self.keys);
        }

        fn sign(&self, manifest: &[u8], keys: &minisign::KeyPair) {
            let signature =
                minisign::sign(None, &keys.sk, std::io::Cursor::new(manifest), None, None).unwrap();
            self.feed.put(MANIFEST, manifest);
            self.feed.put(SIGNATURE, signature.into_string().as_bytes());
        }

        fn updater(&self, current: &str) -> Updater {
            let mut config = Config::new(
                "https://updates.example.com/app",
                &self.keys.pk.to_base64(),
                current,
                self.dir.path().join("staging"),
            )
            .unwrap();
            config.target = "windows-x86_64".into();
            Updater::with_transport(config, self.feed.clone())
        }
    }

    #[test]
    fn a_newer_release_is_found_downloaded_and_kept() {
        let fixture = Fixture::new();
        fixture.publish(
            "0.2.0",
            "windows-x86_64",
            "Example 0.2.0 setup.exe",
            INSTALLER,
        );
        let updater = fixture.updater("0.1.0");

        let update = updater.check().unwrap().expect("0.2.0 is newer");
        assert_eq!(update.version, Version::new(0, 2, 0));
        assert_eq!(update.notes.as_deref(), Some("Faster."));

        let mut heard = Vec::new();
        let staged = updater
            .download(&update, |received, total| heard.push((received, total)))
            .unwrap();
        assert_eq!(std::fs::read(staged.path()).unwrap(), INSTALLER);
        assert_eq!(
            staged.path().file_name().unwrap(),
            "Example_0.2.0_setup.exe"
        );
        let size = INSTALLER.len() as u64;
        assert_eq!(heard.last(), Some(&(size, size)));

        // A second run finds it there and fetches nothing.
        let before = fixture.feed.gets.load(Ordering::Relaxed);
        let again = updater.download(&update, |_, _| {}).unwrap();
        assert_eq!(again.path(), staged.path());
        assert_eq!(fixture.feed.gets.load(Ordering::Relaxed), before);
    }

    #[test]
    fn the_same_or_an_older_release_is_no_update() {
        let fixture = Fixture::new();
        fixture.publish("0.2.0", "windows-x86_64", "setup.exe", INSTALLER);
        assert!(fixture.updater("0.2.0").check().unwrap().is_none());
        assert!(fixture.updater("0.3.0").check().unwrap().is_none());
        assert!(fixture.updater("0.2.0-beta.1").check().unwrap().is_some());
    }

    #[test]
    fn a_release_for_other_platforms_is_no_update() {
        let fixture = Fixture::new();
        fixture.publish("0.2.0", "macos-aarch64", "Example.app.tar.gz", INSTALLER);
        assert!(fixture.updater("0.1.0").check().unwrap().is_none());
    }

    #[test]
    fn a_manifest_someone_else_signed_or_changed_is_refused() {
        let fixture = Fixture::new();
        fixture.publish("0.2.0", "windows-x86_64", "setup.exe", INSTALLER);
        let updater = fixture.updater("0.1.0");

        let ours = fixture.feed.files.lock().unwrap()[MANIFEST].clone();
        let changed = String::from_utf8(ours.clone())
            .unwrap()
            .replace("0.2.0", "0.9.0");
        fixture.feed.put(MANIFEST, changed.as_bytes());
        assert!(matches!(updater.check(), Err(Error::Signature)));

        let theirs = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        fixture.sign(&ours, &theirs);
        assert!(matches!(updater.check(), Err(Error::Signature)));

        fixture.feed.put(SIGNATURE, b"not a signature");
        assert!(matches!(updater.check(), Err(Error::Signature)));
    }

    #[test]
    fn a_download_that_differs_from_the_manifest_is_refused_and_removed() {
        let fixture = Fixture::new();
        fixture.publish("0.2.0", "windows-x86_64", "setup.exe", INSTALLER);
        let updater = fixture.updater("0.1.0");
        let update = updater.check().unwrap().unwrap();

        let mut other = INSTALLER.to_vec();
        other[0] ^= 1;
        let mut longer = INSTALLER.to_vec();
        longer.push(0);
        for body in [&other[..], &longer[..], &INSTALLER[..5]] {
            fixture.feed.put("setup.exe", body);
            let refused = updater.download(&update, |_, _| {});
            assert!(
                matches!(refused, Err(Error::Mismatch | Error::Transport(_))),
                "{refused:?}"
            );
            let folder = fixture.dir.path().join("staging").join("0.2.0");
            assert_eq!(std::fs::read_dir(folder).unwrap().count(), 0);
        }
    }

    #[test]
    fn an_artifact_elsewhere_must_be_https() {
        let fixture = Fixture::new();
        let updater = fixture.updater("0.1.0");
        fixture.publish(
            "0.2.0",
            "windows-x86_64",
            "http://updates.example.com/app/setup.exe",
            INSTALLER,
        );
        assert!(matches!(updater.check(), Err(Error::Url(_))));
        fixture.publish(
            "0.2.0",
            "windows-x86_64",
            "https://cdn.example.com/app/setup.exe",
            INSTALLER,
        );
        assert!(updater.check().unwrap().is_some());
    }

    #[test]
    fn a_new_download_clears_what_older_ones_left() {
        let fixture = Fixture::new();
        let stale = fixture.dir.path().join("staging").join("0.1.5");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("setup.exe"), b"old").unwrap();
        fixture.publish("0.2.0", "windows-x86_64", "setup.exe", INSTALLER);
        let updater = fixture.updater("0.1.0");
        let update = updater.check().unwrap().unwrap();
        updater.download(&update, |_, _| {}).unwrap();
        assert!(!stale.exists());
    }

    #[test]
    fn a_feed_must_be_https_or_this_computer() {
        let key = minisign::KeyPair::generate_unencrypted_keypair()
            .unwrap()
            .pk
            .to_base64();
        let config = |feed| Config::new(feed, &key, "1.0.0", "staging");
        assert!(matches!(
            config("http://updates.example.com/"),
            Err(Error::Url(_))
        ));
        assert!(matches!(config("file:///tmp/feed/"), Err(Error::Url(_))));
        assert!(config("http://127.0.0.1:8000/feed").is_ok());
        assert_eq!(
            config("https://updates.example.com/app")
                .unwrap()
                .feed
                .as_str(),
            "https://updates.example.com/app/"
        );
    }

    #[test]
    fn an_unknown_kind_reads_and_is_no_update() {
        let fixture = Fixture::new();
        let manifest = format!(
            r#"{{"version":"0.2.0","artifacts":{{"windows-x86_64":{{"kind":"msix","url":"a","sha256":"{}","size":1}}}}}}"#,
            "0".repeat(64)
        );
        fixture.sign(manifest.as_bytes(), &fixture.keys);
        assert!(fixture.updater("0.1.0").check().unwrap().is_none());
    }

    fn announce(fixture: &Fixture, artifacts: &[(&str, Kind, &str)]) {
        let manifest = Manifest {
            version: Version::new(0, 2, 0),
            notes: None,
            artifacts: artifacts
                .iter()
                .map(|(target, kind, url)| {
                    fixture.feed.put(url, b"a package");
                    (
                        (*target).to_owned(),
                        Artifact {
                            kind: *kind,
                            url: (*url).to_owned(),
                            sha256: hex(&Sha256::digest(b"a package")),
                            size: 9,
                        },
                    )
                })
                .collect(),
        };
        fixture.sign(&serde_json::to_vec(&manifest).unwrap(), &fixture.keys);
    }

    #[test]
    fn a_packaged_copy_hears_of_its_package_and_never_downloads_it() {
        let fixture = Fixture::new();
        announce(
            &fixture,
            &[
                ("linux-x86_64", Kind::AppImage, "App-0.2.0-x86_64.AppImage"),
                ("linux-x86_64-deb", Kind::Deb, "app_0.2.0-1_amd64.deb"),
                (
                    "linux-x86_64-pacman",
                    Kind::Pacman,
                    "app-0.2.0-1-x86_64.pkg.tar.zst",
                ),
            ],
        );
        let updater = |package| {
            let mut updater = fixture.updater("0.1.0");
            updater.config.target = "linux-x86_64".into();
            updater.config.package = package;
            updater
        };

        // An AppImage gets the AppImage.
        let update = updater(None).check().unwrap().unwrap();
        assert_eq!(update.managed_by(), None);

        for manager in [PackageManager::Apt, PackageManager::Pacman] {
            let updater = updater(Some(manager));
            let update = updater.check().unwrap().expect("0.2.0 is announced");
            assert_eq!(update.version, Version::new(0, 2, 0));
            assert_eq!(update.managed_by(), Some(manager));
            let before = fixture.feed.gets.load(Ordering::Relaxed);
            let refused = updater.download(&update, |_, _| {});
            assert!(
                matches!(refused, Err(Error::ManagedBySystem { manager: found }) if found == manager),
                "{refused:?}"
            );
            assert_eq!(fixture.feed.gets.load(Ordering::Relaxed), before);
        }
        let message = Error::ManagedBySystem {
            manager: PackageManager::Apt,
        }
        .to_string();
        assert!(message.contains("update it with apt"), "{message}");
    }

    #[test]
    fn a_packaged_copy_without_its_package_in_the_feed_hears_nothing() {
        let fixture = Fixture::new();
        announce(
            &fixture,
            &[
                ("linux-x86_64", Kind::AppImage, "App-0.2.0-x86_64.AppImage"),
                // A package under the AppImage's key is not one to offer.
                ("linux-aarch64", Kind::Deb, "app_0.2.0-1_arm64.deb"),
            ],
        );
        let mut updater = fixture.updater("0.1.0");
        updater.config.target = "linux-x86_64".into();
        updater.config.package = Some(PackageManager::Apt);
        assert!(updater.check().unwrap().is_none());
        updater.config.target = "linux-aarch64".into();
        updater.config.package = None;
        assert!(updater.check().unwrap().is_none());
    }

    #[test]
    fn package_kinds_read_as_unknown_to_older_versions() {
        // What an older version sees: a kind it has no variant for.
        #[derive(serde::Deserialize, PartialEq, Debug)]
        #[serde(rename_all = "kebab-case")]
        enum Older {
            Nsis,
            AppTarGz,
            #[serde(rename = "appimage")]
            AppImage,
            #[serde(other)]
            Unknown,
        }
        for kind in [Kind::Deb, Kind::Pacman] {
            let text = serde_json::to_string(&kind).unwrap();
            assert_eq!(
                serde_json::from_str::<Older>(&text).unwrap(),
                Older::Unknown
            );
            assert_eq!(serde_json::from_str::<Kind>(&text).unwrap(), kind);
        }
        assert_eq!(serde_json::to_string(&Kind::Deb).unwrap(), r#""deb""#);
        assert_eq!(serde_json::to_string(&Kind::Pacman).unwrap(), r#""pacman""#);
    }

    #[test]
    fn an_appimage_is_found_for_linux_and_kept_with_its_hash() {
        let fixture = Fixture::new();
        let body = b"\x7fELF\x02\x01\x01\0AI\x02 an AppImage";
        fixture.feed.put("Example-0.2.0-x86_64.AppImage", body);
        let manifest = Manifest {
            version: Version::new(0, 2, 0),
            notes: None,
            artifacts: BTreeMap::from([(
                "linux-x86_64".to_owned(),
                Artifact {
                    kind: Kind::AppImage,
                    url: "Example-0.2.0-x86_64.AppImage".into(),
                    sha256: hex(&Sha256::digest(body)),
                    size: body.len() as u64,
                },
            )]),
        };
        let bytes = serde_json::to_vec(&manifest).unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains(r#""kind":"appimage""#));
        fixture.sign(&bytes, &fixture.keys);

        // Windows finds nothing for itself in it.
        assert!(fixture.updater("0.1.0").check().unwrap().is_none());

        let mut updater = fixture.updater("0.1.0");
        updater.config.target = "linux-x86_64".into();
        let update = updater.check().unwrap().expect("0.2.0 is newer");
        let staged = updater.download(&update, |_, _| {}).unwrap();
        assert_eq!(std::fs::read(staged.path()).unwrap(), body);
        assert_eq!(staged.kind, Kind::AppImage);
        assert_eq!(staged.sha256, hex(&Sha256::digest(body)));
        assert_eq!(staged.artifact, "Example-0.2.0-x86_64.AppImage");
        assert_eq!(staged.running, Version::new(0, 1, 0));
    }
}
