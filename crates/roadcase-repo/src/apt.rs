//! An apt repository as a static folder:
//!
//! ```text
//! <dir>/
//!   key.asc, key.gpg                          the public key, for people and for Signed-By
//!   pool/<component>/<p>/<package>/<package>_<version>_<arch>.deb
//!   dists/<suite>/Release, Release.gpg, InRelease
//!   dists/<suite>/<component>/binary-<arch>/Packages, Packages.gz
//! ```
//!
//! The pool is the truth: [`index`] reads every `.deb` in it, keeps the
//! newest [`Apt::keep`] versions of each package and architecture (removing
//! older files), and writes `dists/<suite>` again from what is left.

use crate::deb::{Control, read_control};
use crate::error::io;
use crate::key::SigningKey;
use crate::version::compare_debian;
use crate::{Error, Result, files};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// How the repository describes itself.
#[derive(Debug, Clone)]
pub struct Apt {
    /// The suite, such as `stable`: `dists/<suite>`, and what a sources
    /// file's `Suites` names.
    pub suite: String,
    /// `Codename`: the suite when absent.
    pub codename: Option<String>,
    /// `Origin`.
    pub origin: Option<String>,
    /// `Label`.
    pub label: Option<String>,
    /// `Description`.
    pub description: Option<String>,
    /// How many versions of each package and architecture stay listed (and in
    /// the pool). At least one.
    pub keep: usize,
}

impl Default for Apt {
    fn default() -> Self {
        Self {
            suite: "stable".into(),
            codename: None,
            origin: None,
            label: None,
            description: None,
            keep: 3,
        }
    }
}

/// What [`index`] found and did.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// Listed packages: component, package, version, architecture.
    pub listed: Vec<(String, String, String, String)>,
    /// Files removed from the pool, beyond [`Apt::keep`].
    pub removed: Vec<PathBuf>,
}

/// Where a package goes in the pool: `pool/<component>/<p>/<package>/`, `<p>`
/// being its first letter, or its first four for a `lib` package.
pub fn pool_folder(component: &str, package: &str) -> String {
    let prefix = if package.starts_with("lib") && package.len() > 3 {
        &package[..4]
    } else {
        &package[..1]
    };
    format!("pool/{component}/{prefix}/{package}")
}

/// Copies `.deb` files into the pool of `component`, named
/// `<package>_<version>_<arch>.deb` (the version without its epoch).
/// A file already there with other contents is refused: a published version
/// never changes.
///
/// # Errors
///
/// [`Error::Package`] when a file is not a `.deb` or would replace another;
/// [`Error::Invalid`] for a component that is not a plain name; [`Error::Io`].
pub fn add(dir: &Path, component: &str, debs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    files::check_name("the component", component)?;
    let mut placed = Vec::new();
    for deb in debs {
        let control = read_control(deb)?;
        let package = control.package()?;
        files::check_name("the package", package)?;
        let version = control.version()?;
        let version = version.split_once(':').map_or(version, |(_, rest)| rest);
        let arch = control.architecture()?;
        let folder = dir.join(pool_folder(component, package));
        let to = folder.join(format!("{package}_{version}_{arch}.deb"));
        files::place(deb, &to)?;
        placed.push(to);
    }
    Ok(placed)
}

struct Found {
    component: String,
    control: Control,
    package: String,
    version: String,
    arch: String,
    path: PathBuf,
}

/// Reads the pool, applies [`Apt::keep`], and writes `dists/<suite>` signed
/// with `key`, and the public key as `key.asc` and `key.gpg`.
///
/// # Errors
///
/// [`Error::Invalid`] for an empty pool or `keep` of zero; [`Error::Package`]
/// for a file that does not read, or two files of one version;
/// [`Error::Key`]; [`Error::Io`].
pub fn index(dir: &Path, apt: &Apt, key: &SigningKey) -> Result<Report> {
    if apt.keep == 0 {
        return Err(Error::Invalid("keep at least one version".into()));
    }
    files::check_name("the suite", &apt.suite)?;
    let pool = dir.join("pool");
    let mut found = Vec::new();
    for component in files::folders(&pool)? {
        for path in files::walk(&pool.join(&component))? {
            if path.extension().is_none_or(|extension| extension != "deb") {
                continue;
            }
            let control = read_control(&path)?;
            found.push(Found {
                package: control.package()?.to_owned(),
                version: control.version()?.to_owned(),
                arch: control.architecture()?.to_owned(),
                component: component.clone(),
                control,
                path,
            });
        }
    }
    if found.is_empty() {
        return Err(Error::Invalid(format!(
            "{} holds no .deb: add one first",
            pool.display()
        )));
    }

    let mut groups: BTreeMap<(String, String, String), Vec<Found>> = BTreeMap::new();
    for item in found {
        groups
            .entry((
                item.component.clone(),
                item.package.clone(),
                item.arch.clone(),
            ))
            .or_default()
            .push(item);
    }
    let mut report = Report::default();
    let mut kept = Vec::new();
    for ((component, package, arch), mut items) in groups {
        items.sort_by(|a, b| compare_debian(&b.version, &a.version));
        for pair in items.windows(2) {
            if compare_debian(&pair[0].version, &pair[1].version).is_eq() {
                return Err(Error::Package(format!(
                    "{component}: {package} {} {arch} is in the pool twice: {} and {}",
                    pair[0].version,
                    pair[0].path.display(),
                    pair[1].path.display()
                )));
            }
        }
        for old in items.split_off(apt.keep.min(items.len())) {
            std::fs::remove_file(&old.path).map_err(io(&old.path))?;
            report.removed.push(old.path);
        }
        kept.extend(items);
    }

    let mut architectures: BTreeSet<String> = kept
        .iter()
        .filter(|item| item.arch != "all")
        .map(|item| item.arch.clone())
        .collect();
    if architectures.is_empty() {
        architectures.insert("all".into());
    }
    let components: BTreeSet<String> = kept.iter().map(|item| item.component.clone()).collect();

    let dists = dir.join("dists").join(&apt.suite);
    if dists.exists() {
        std::fs::remove_dir_all(&dists).map_err(io(&dists))?;
    }
    let mut hashes = Vec::new();
    for component in &components {
        for arch in &architectures {
            let mut text = String::new();
            for item in kept.iter().filter(|item| {
                &item.component == component && (&item.arch == arch || item.arch == "all")
            }) {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&stanza(dir, item)?);
            }
            let relative = format!("{component}/binary-{arch}/Packages");
            let plain = text.into_bytes();
            let gzip = files::gzip(&plain)?;
            for (name, bytes) in [(relative.clone(), plain), (format!("{relative}.gz"), gzip)] {
                files::write(&dists.join(&name), &bytes)?;
                hashes.push((name, files::sha256(&bytes), bytes.len()));
            }
        }
    }
    for item in &kept {
        report.listed.push((
            item.component.clone(),
            item.package.clone(),
            item.version.clone(),
            item.arch.clone(),
        ));
    }

    let release = release(apt, &architectures, &components, &hashes);
    files::write(&dists.join("Release"), release.as_bytes())?;
    files::write(
        &dists.join("Release.gpg"),
        key.detached_armored(release.as_bytes())?.as_bytes(),
    )?;
    files::write(
        &dists.join("InRelease"),
        key.clearsign(&release)?.as_bytes(),
    )?;
    let public = key.public();
    files::write(&dir.join("key.asc"), public.armored()?.as_bytes())?;
    files::write(&dir.join("key.gpg"), &public.binary()?)?;
    Ok(report)
}

/// The package's `Packages` stanza: its control file, then where it is.
fn stanza(dir: &Path, item: &Found) -> Result<String> {
    let (sha256, size) = files::digest(&item.path)?;
    let relative = item
        .path
        .strip_prefix(dir)
        .map_err(|_| Error::Invalid(format!("{} is not in the repository", item.path.display())))?;
    let filename = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    let mut control = item.control.clone();
    control.fields.retain(|(key, _)| {
        !["Filename", "Size", "MD5sum", "SHA1", "SHA256", "SHA512"]
            .iter()
            .any(|ours| ours.eq_ignore_ascii_case(key))
    });
    // Before the description, as dpkg-scanpackages places them.
    let at = control
        .fields
        .iter()
        .position(|(key, _)| key.eq_ignore_ascii_case("Description"))
        .unwrap_or(control.fields.len());
    control.fields.splice(
        at..at,
        [
            ("Filename".to_owned(), filename),
            ("Size".to_owned(), size.to_string()),
            ("SHA256".to_owned(), sha256),
        ],
    );
    Ok(control.write())
}

fn release(
    apt: &Apt,
    architectures: &BTreeSet<String>,
    components: &BTreeSet<String>,
    hashes: &[(String, String, usize)],
) -> String {
    let mut text = String::new();
    let mut field = |key: &str, value: &str| {
        text.push_str(&format!("{key}: {value}\n"));
    };
    if let Some(origin) = &apt.origin {
        field("Origin", origin);
    }
    if let Some(label) = &apt.label {
        field("Label", label);
    }
    field("Suite", &apt.suite);
    field("Codename", apt.codename.as_deref().unwrap_or(&apt.suite));
    field("Date", &crate::date::rfc2822(crate::date::now()));
    field(
        "Architectures",
        &architectures.iter().cloned().collect::<Vec<_>>().join(" "),
    );
    field(
        "Components",
        &components.iter().cloned().collect::<Vec<_>>().join(" "),
    );
    if let Some(description) = &apt.description {
        field("Description", description);
    }
    text.push_str("SHA256:\n");
    for (name, sha256, size) in hashes {
        text.push_str(&format!(" {sha256} {size:>16} {name}\n"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deb::tests::deb;
    use crate::key::{PublicKey, generate};

    fn package(dir: &Path, name: &str, version: &str, arch: &str) -> PathBuf {
        let path = dir.join(format!("{name}-{version}-{arch}.deb"));
        let control = format!(
            "Package: {name}\nVersion: {version}\nArchitecture: {arch}\nMaintainer: Example <a@example.com>\nDescription: An example\n More of it.\n"
        );
        std::fs::write(&path, deb(&control)).unwrap();
        path
    }

    #[test]
    fn a_repository_lists_signs_and_keeps_what_it_should() {
        let dir = tempfile::tempdir().unwrap();
        let built = dir.path().join("built");
        std::fs::create_dir(&built).unwrap();
        let repo = dir.path().join("repo");
        let pair = generate("Example Packages <packages@example.com>", None).unwrap();
        let key = SigningKey::parse(&pair.secret, None).unwrap();
        let public = PublicKey::parse(pair.public.as_bytes()).unwrap();
        let apt = Apt {
            origin: Some("Example".into()),
            label: Some("Example".into()),
            keep: 2,
            ..Apt::default()
        };

        let placed = add(
            &repo,
            "main",
            &[
                package(&built, "example", "1.0.0-1", "amd64"),
                package(&built, "example", "1.0.0-1", "arm64"),
                package(&built, "example-data", "1.0.0-1", "all"),
            ],
        )
        .unwrap();
        assert_eq!(
            placed[0],
            repo.join("pool/main/e/example/example_1.0.0-1_amd64.deb")
        );
        index(&repo, &apt, &key).unwrap();
        for version in ["1.1.0~beta.1-1", "1.1.0-1"] {
            add(
                &repo,
                "main",
                &[package(&built, "example", version, "amd64")],
            )
            .unwrap();
            index(&repo, &apt, &key).unwrap();
        }
        // The same version again, unchanged, is fine; changed, it is refused.
        add(
            &repo,
            "main",
            &[package(&built, "example", "1.1.0-1", "amd64")],
        )
        .unwrap();
        let changed = built.join("changed.deb");
        std::fs::write(
            &changed,
            deb("Package: example\nVersion: 1.1.0-1\nArchitecture: amd64\nDescription: Other\n"),
        )
        .unwrap();
        assert!(add(&repo, "main", &[changed]).is_err());

        let report = index(&repo, &apt, &key).unwrap();
        assert!(report.removed.is_empty());
        let pool = repo.join("pool/main/e/example");
        assert!(!pool.join("example_1.0.0-1_amd64.deb").exists());
        assert!(pool.join("example_1.0.0-1_arm64.deb").exists());

        let dists = repo.join("dists/stable");
        let release = std::fs::read_to_string(dists.join("Release")).unwrap();
        assert!(release.starts_with(
            "Origin: Example\nLabel: Example\nSuite: stable\nCodename: stable\nDate: "
        ));
        assert!(release.contains("\nArchitectures: amd64 arm64\nComponents: main\nSHA256:\n"));
        let packages = std::fs::read_to_string(dists.join("main/binary-amd64/Packages")).unwrap();
        let versions: Vec<_> = packages
            .lines()
            .filter_map(|line| line.strip_prefix("Version: "))
            .collect();
        assert_eq!(versions, ["1.1.0-1", "1.1.0~beta.1-1", "1.0.0-1"]);
        assert!(
            packages.contains("Filename: pool/main/e/example/example_1.1.0-1_amd64.deb\nSize: ")
        );
        assert!(packages.contains("Package: example-data\n"));
        let arm = std::fs::read_to_string(dists.join("main/binary-arm64/Packages")).unwrap();
        assert!(arm.contains("example_1.0.0-1_arm64.deb") && arm.contains("example-data"));

        // Every index is in Release with its hash and size.
        for name in [
            "main/binary-amd64/Packages",
            "main/binary-amd64/Packages.gz",
            "main/binary-arm64/Packages",
        ] {
            let bytes = std::fs::read(dists.join(name)).unwrap();
            let line = format!(" {} {:>16} {name}\n", files::sha256(&bytes), bytes.len());
            assert!(release.contains(&line), "{line}");
        }
        let gz = std::fs::read(dists.join("main/binary-amd64/Packages.gz")).unwrap();
        assert_eq!(
            crate::archive::decompress(&gz).unwrap(),
            packages.as_bytes()
        );

        public
            .verify_detached(
                release.as_bytes(),
                &std::fs::read(dists.join("Release.gpg")).unwrap(),
            )
            .unwrap();
        let inline = std::fs::read_to_string(dists.join("InRelease")).unwrap();
        assert_eq!(
            public.verify_cleartext(&inline).unwrap().trim_end(),
            release.trim_end()
        );
        let exported = PublicKey::parse(&std::fs::read(repo.join("key.gpg")).unwrap()).unwrap();
        assert_eq!(exported.fingerprint(), pair.fingerprint);

        // Keeping one removes the beta.
        let report = index(&repo, &Apt { keep: 1, ..apt }, &key).unwrap();
        assert_eq!(report.removed.len(), 1);
        assert!(!pool.join("example_1.1.0~beta.1-1_amd64.deb").exists());
    }

    #[test]
    fn an_empty_pool_is_no_repository() {
        let dir = tempfile::tempdir().unwrap();
        let pair = generate("Example <a@example.com>", None).unwrap();
        let key = SigningKey::parse(&pair.secret, None).unwrap();
        assert!(index(dir.path(), &Apt::default(), &key).is_err());
        assert!(add(dir.path(), "../main", &[]).is_err());
        assert_eq!(
            pool_folder("main", "libexample"),
            "pool/main/libe/libexample"
        );
    }
}
