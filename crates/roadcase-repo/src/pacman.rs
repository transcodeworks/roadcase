//! A pacman repository as a static folder, one sub-folder per architecture,
//! as a `Server = https://<host>/<path>/$arch` line reads it:
//!
//! ```text
//! <dir>/
//!   key.asc                                     the public key, for pacman-key --add
//!   <arch>/<pkgname>-<pkgver>-<pkgrel>-<arch>.pkg.tar.zst, and its .sig
//!   <arch>/<repo>.db, <repo>.db.tar.gz          what pacman -Sy reads
//!   <arch>/<repo>.files, <repo>.files.tar.gz    what pacman -F reads
//!   <arch>/<repo>.db.sig, ...                   a signature of each
//! ```
//!
//! What `repo-add --sign` makes, without `repo-add` or `gpg`. The `.db` and
//! `.files` are copies of their `.tar.gz`, not links, since static hosts serve
//! no links. The packages are the truth: [`index`] reads every package in
//! an architecture's folder, keeps the newest [`Pacman::keep`] versions of each
//! (removing older files), signs those without a valid signature, and lists
//! the newest of each in the database, as pacman holds one version per name.

use crate::archive::{decompress, tar_entries};
use crate::error::io;
use crate::key::SigningKey;
use crate::version::compare_pacman;
use crate::{Error, Result, files};
use base64::Engine;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The repository.
#[derive(Debug, Clone)]
pub struct Pacman {
    /// Its name: `[<name>]` in pacman.conf, and `<name>.db`.
    pub name: String,
    /// How many versions of each package stay in the folder: the newest is
    /// listed, older ones stay for the links that still name them (an AUR
    /// `PKGBUILD` not yet updated, a mirror behind). At least one.
    pub keep: usize,
}

/// A package's `.PKGINFO`, and the files it installs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    /// `.PKGINFO`'s fields in order; a key may repeat (`depend`, `license`).
    pub info: Vec<(String, String)>,
    /// The paths it installs, without a leading `/`; folders end in `/`.
    pub files: Vec<String>,
}

impl Package {
    /// The first value of a `.PKGINFO` key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.info
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    /// Every value of a `.PKGINFO` key.
    pub fn all(&self, key: &str) -> Vec<&str> {
        self.info
            .iter()
            .filter(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
            .collect()
    }

    fn required(&self, key: &str) -> Result<&str> {
        self.get(key)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::Package(format!("its .PKGINFO has no {key}")))
    }

    /// `pkgname`.
    ///
    /// # Errors
    ///
    /// [`Error::Package`] when it is missing.
    pub fn name(&self) -> Result<&str> {
        self.required("pkgname")
    }

    /// `pkgver`: `[epoch:]version-release`.
    ///
    /// # Errors
    ///
    /// [`Error::Package`] when it is missing.
    pub fn version(&self) -> Result<&str> {
        self.required("pkgver")
    }

    /// `arch`.
    ///
    /// # Errors
    ///
    /// [`Error::Package`] when it is missing.
    pub fn arch(&self) -> Result<&str> {
        self.required("arch")
    }

    /// Reads `.PKGINFO`'s `key = value` lines.
    ///
    /// # Errors
    ///
    /// [`Error::Package`] for a line that is not one.
    pub fn parse_info(text: &str) -> Result<Vec<(String, String)>> {
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(|line| {
                line.split_once(" = ")
                    .or_else(|| line.split_once('='))
                    .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
                    .ok_or_else(|| {
                        Error::Package(format!("the .PKGINFO line {line:?} is not key = value"))
                    })
            })
            .collect()
    }
}

/// Whether a file name is a package's: `*.pkg.tar`, `.gz`, `.xz` or `.zst`.
pub fn is_package(name: &str) -> bool {
    [".pkg.tar", ".pkg.tar.gz", ".pkg.tar.xz", ".pkg.tar.zst"]
        .iter()
        .any(|extension| name.ends_with(extension))
}

/// Reads the package at `path`.
///
/// # Errors
///
/// [`Error::Io`]; [`Error::Package`] when it is not a pacman package.
pub fn read_package(path: &Path) -> Result<Package> {
    let bytes = std::fs::read(path).map_err(io(path))?;
    package_of(&bytes).map_err(|error| match error {
        Error::Package(message) => Error::Package(format!("{}: {message}", path.display())),
        other => other,
    })
}

pub(crate) fn package_of(bytes: &[u8]) -> Result<Package> {
    let tar = decompress(bytes)?;
    let entries = tar_entries(&tar, |path| path == ".PKGINFO")?;
    let mut info = None;
    let mut files = Vec::new();
    for entry in entries {
        if entry.path == ".PKGINFO" {
            info = entry.contents;
            continue;
        }
        // .PKGINFO, .MTREE, .BUILDINFO, .INSTALL, .CHANGELOG: about the package.
        if entry.path.starts_with('.') && !entry.path.contains('/') {
            continue;
        }
        files.push(if entry.folder {
            format!("{}/", entry.path)
        } else {
            entry.path
        });
    }
    let info = info.ok_or_else(|| Error::Package("it has no .PKGINFO".into()))?;
    files.sort();
    Ok(Package {
        info: Package::parse_info(&String::from_utf8_lossy(&info))?,
        files,
    })
}

/// Copies packages into their architecture's folder of the repository. A
/// package for `any` goes into every architecture's folder there is.
///
/// # Errors
///
/// [`Error::Package`] when a file is not a package or would replace another
/// of the same name; [`Error::Invalid`] for an `any` package and no
/// architecture yet; [`Error::Io`].
pub fn add(dir: &Path, packages: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut placed = Vec::new();
    for path in packages {
        let package = read_package(path)?;
        let arch = package.arch()?;
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| is_package(name))
            .ok_or_else(|| {
                Error::Package(format!(
                    "{} is not named as a package (*.pkg.tar.zst)",
                    path.display()
                ))
            })?;
        let arches = if arch == "any" {
            let arches = files::folders(dir)?;
            if arches.is_empty() {
                return Err(Error::Invalid(format!(
                    "{name} is for any architecture, and {} has none yet: add a package of one first",
                    dir.display()
                )));
            }
            arches
        } else {
            files::check_name("the architecture", arch)?;
            vec![arch.to_owned()]
        };
        for arch in arches {
            let to = dir.join(arch).join(&name);
            files::place(path, &to)?;
            placed.push(to);
        }
    }
    Ok(placed)
}

/// What [`index`] found and did.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// Listed packages: architecture, name, version.
    pub listed: Vec<(String, String, String)>,
    /// Signatures made.
    pub signed: Vec<PathBuf>,
    /// Files removed, beyond [`Pacman::keep`].
    pub removed: Vec<PathBuf>,
}

struct Found {
    path: PathBuf,
    file: String,
    package: Package,
    name: String,
    version: String,
}

/// Reads each architecture's packages, applies [`Pacman::keep`], signs, and
/// writes the databases signed with `key`, and the public key as `key.asc`.
///
/// # Errors
///
/// [`Error::Invalid`] for a repository with no packages or `keep` of zero;
/// [`Error::Package`]; [`Error::Key`]; [`Error::Io`].
pub fn index(dir: &Path, pacman: &Pacman, key: &SigningKey) -> Result<Report> {
    files::check_name("the repository's name", &pacman.name)?;
    if pacman.keep == 0 {
        return Err(Error::Invalid("keep at least one version".into()));
    }
    let public = key.public();
    let mut report = Report::default();
    let mut any = false;
    for arch in files::folders(dir)? {
        let folder = dir.join(&arch);
        let mut groups: BTreeMap<String, Vec<Found>> = BTreeMap::new();
        for path in files::walk(&folder)? {
            let file = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            if path.parent() != Some(folder.as_path()) || !is_package(&file) {
                continue;
            }
            let package = read_package(&path)?;
            let name = package.name()?.to_owned();
            let version = package.version()?.to_owned();
            groups.entry(name.clone()).or_default().push(Found {
                path,
                file,
                package,
                name,
                version,
            });
        }
        if groups.is_empty() {
            continue;
        }
        any = true;
        let mut newest = Vec::new();
        for (name, mut items) in groups {
            items.sort_by(|a, b| compare_pacman(&b.version, &a.version));
            for pair in items.windows(2) {
                if compare_pacman(&pair[0].version, &pair[1].version).is_eq() {
                    return Err(Error::Package(format!(
                        "{arch}: {name} {} is there twice: {} and {}",
                        pair[0].version, pair[0].file, pair[1].file
                    )));
                }
            }
            for old in items.split_off(pacman.keep.min(items.len())) {
                for path in [old.path.clone(), signature_path(&old.path)] {
                    if path.exists() {
                        std::fs::remove_file(&path).map_err(io(&path))?;
                        report.removed.push(path);
                    }
                }
            }
            for item in &items {
                let signature = signature_path(&item.path);
                let valid = std::fs::read(&signature).is_ok_and(|bytes| {
                    std::fs::read(&item.path)
                        .is_ok_and(|data| public.verify_detached(&data, &bytes).is_ok())
                });
                if !valid {
                    let data = std::fs::read(&item.path).map_err(io(&item.path))?;
                    files::write(&signature, &key.detached(&data)?)?;
                    report.signed.push(signature);
                }
            }
            newest.push(items.swap_remove(0));
        }

        let mut db = Vec::new();
        let mut files_db = Vec::new();
        for item in &newest {
            let signature = std::fs::read(signature_path(&item.path))
                .map_err(io(signature_path(&item.path)))?;
            let (sha256, size) = files::digest(&item.path)?;
            let folder_name = format!("{}-{}", item.name, item.version);
            let desc = desc(&item.package, &item.file, size, &sha256, &signature);
            let listing = files_listing(&item.package);
            db.push((folder_name.clone(), vec![("desc", desc.clone())]));
            files_db.push((folder_name, vec![("desc", desc), ("files", listing)]));
            report
                .listed
                .push((arch.clone(), item.name.clone(), item.version.clone()));
        }
        for (kind, entries) in [("db", db), ("files", files_db)] {
            let archive = files::gzip(&database(&entries)?)?;
            let signature = key.detached(&archive)?;
            for name in [
                format!("{}.{kind}", pacman.name),
                format!("{}.{kind}.tar.gz", pacman.name),
            ] {
                files::write(&folder.join(&name), &archive)?;
                files::write(&folder.join(format!("{name}.sig")), &signature)?;
            }
        }
    }
    if !any {
        return Err(Error::Invalid(format!(
            "{} holds no package in an architecture's folder: add one first",
            dir.display()
        )));
    }
    files::write(&dir.join("key.asc"), public.armored()?.as_bytes())?;
    Ok(report)
}

fn signature_path(package: &Path) -> PathBuf {
    let mut name = package.as_os_str().to_owned();
    name.push(".sig");
    PathBuf::from(name)
}

/// A database entry's `desc`, as `repo-add` writes it.
pub fn desc(package: &Package, file: &str, size: u64, sha256: &str, signature: &[u8]) -> String {
    let mut text = String::new();
    let mut field = |key: &str, values: Vec<&str>| {
        let values: Vec<&str> = values
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect();
        if values.is_empty() {
            return;
        }
        text.push_str(&format!("%{key}%\n"));
        for value in values {
            text.push_str(value);
            text.push('\n');
        }
        text.push('\n');
    };
    let size = size.to_string();
    let signature = base64::engine::general_purpose::STANDARD.encode(signature);
    let name = package.get("pkgname").unwrap_or_default();
    field("FILENAME", vec![file]);
    field("NAME", vec![name]);
    field("BASE", vec![package.get("pkgbase").unwrap_or(name)]);
    field("VERSION", package.all("pkgver"));
    field("DESC", package.all("pkgdesc"));
    field("GROUPS", package.all("group"));
    field("CSIZE", vec![&size]);
    field("ISIZE", package.all("size"));
    field("SHA256SUM", vec![sha256]);
    field("PGPSIG", vec![&signature]);
    field("URL", package.all("url"));
    field("LICENSE", package.all("license"));
    field("ARCH", package.all("arch"));
    field("BUILDDATE", package.all("builddate"));
    field("PACKAGER", package.all("packager"));
    field("REPLACES", package.all("replaces"));
    field("CONFLICTS", package.all("conflict"));
    field("PROVIDES", package.all("provides"));
    field("DEPENDS", package.all("depend"));
    field("OPTDEPENDS", package.all("optdepend"));
    field("MAKEDEPENDS", package.all("makedepend"));
    field("CHECKDEPENDS", package.all("checkdepend"));
    text
}

fn files_listing(package: &Package) -> String {
    let mut text = String::from("%FILES%\n");
    for file in &package.files {
        text.push_str(file);
        text.push('\n');
    }
    text
}

/// A database's tar: a folder per package, holding its files.
fn database(entries: &[(String, Vec<(&str, String)>)]) -> Result<Vec<u8>> {
    let time = crate::date::now();
    let mut builder = tar::Builder::new(Vec::new());
    let failed = |error: std::io::Error| Error::Invalid(format!("writing the database: {error}"));
    for (folder, members) in entries {
        let mut header = tar::Header::new_ustar();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_mode(0o755);
        header.set_mtime(time);
        header.set_size(0);
        header.set_cksum();
        builder
            .append_data(&mut header, format!("{folder}/"), std::io::empty())
            .map_err(failed)?;
        for (name, text) in members {
            let mut header = tar::Header::new_ustar();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_mode(0o644);
            header.set_mtime(time);
            header.set_size(text.len() as u64);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("{folder}/{name}"), text.as_bytes())
                .map_err(failed)?;
        }
    }
    builder.into_inner().map_err(failed)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::key::{PublicKey, generate};

    /// A package as makepkg makes one, zstd-compressed.
    pub(crate) fn package(name: &str, version: &str, arch: &str) -> Vec<u8> {
        described(name, version, arch, "An example")
    }

    fn described(name: &str, version: &str, arch: &str, description: &str) -> Vec<u8> {
        let info = format!(
            "# Generated by makepkg\npkgname = {name}\npkgbase = {name}\npkgver = {version}\n\
             pkgdesc = {description}\nurl = https://example.com\nbuilddate = 1795435200\n\
             packager = Example <a@example.com>\nsize = 1024\narch = {arch}\nlicense = MIT\n\
             depend = glibc\ndepend = gcc-libs\noptdepend = xdg-utils: open links\n"
        );
        let mut builder = tar::Builder::new(Vec::new());
        let mut add = |path: &str, data: &[u8], folder: bool| {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(if folder {
                tar::EntryType::Directory
            } else {
                tar::EntryType::Regular
            });
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, data).unwrap();
        };
        add(".BUILDINFO", b"format = 2\n", false);
        add(".MTREE", b"", false);
        add(".PKGINFO", info.as_bytes(), false);
        add("opt/", b"", true);
        add(&format!("opt/{name}/"), b"", true);
        add(&format!("opt/{name}/.roadcase-install"), b"pacman\n", false);
        add("usr/bin/", b"", true);
        add(&format!("usr/bin/{name}"), b"", false);
        let tar = builder.into_inner().unwrap();
        ruzstd::encoding::compress_to_vec(&tar[..], ruzstd::encoding::CompressionLevel::Fastest)
    }

    pub(crate) fn file(dir: &Path, name: &str, version: &str, arch: &str) -> PathBuf {
        let path = dir.join(format!("{name}-{version}-{arch}.pkg.tar.zst"));
        std::fs::write(&path, package(name, version, arch)).unwrap();
        path
    }

    fn database_entries(path: &Path) -> BTreeMap<String, String> {
        let tar = decompress(&std::fs::read(path).unwrap()).unwrap();
        tar_entries(&tar, |_| true)
            .unwrap()
            .into_iter()
            .filter_map(|entry| Some((entry.path, String::from_utf8(entry.contents?).unwrap())))
            .collect()
    }

    #[test]
    fn a_package_gives_its_info_and_files() {
        let package = package_of(&package("example", "1.0.0-1", "x86_64")).unwrap();
        assert_eq!(package.name().unwrap(), "example");
        assert_eq!(package.version().unwrap(), "1.0.0-1");
        assert_eq!(package.all("depend"), ["glibc", "gcc-libs"]);
        assert_eq!(
            package.files,
            [
                "opt/",
                "opt/example/",
                "opt/example/.roadcase-install",
                "usr/bin/",
                "usr/bin/example"
            ]
        );
        assert!(is_package("a-1-1-x86_64.pkg.tar.zst"));
        assert!(!is_package("a-1-1-x86_64.pkg.tar.zst.sig"));
    }

    #[test]
    fn a_repository_lists_the_newest_signs_and_keeps_what_it_should() {
        let dir = tempfile::tempdir().unwrap();
        let built = dir.path().join("built");
        std::fs::create_dir(&built).unwrap();
        let repo = dir.path().join("repo");
        let pair = generate("Example Packages <packages@example.com>", None).unwrap();
        let key = SigningKey::parse(&pair.secret, None).unwrap();
        let public = PublicKey::parse(pair.public.as_bytes()).unwrap();
        let pacman = Pacman {
            name: "example".into(),
            keep: 2,
        };

        add(
            &repo,
            &[
                file(&built, "example", "1.0.0-1", "x86_64"),
                file(&built, "example", "1.0.0-1", "aarch64"),
            ],
        )
        .unwrap();
        let report = index(&repo, &pacman, &key).unwrap();
        assert_eq!(report.signed.len(), 2);
        for version in ["1.1.0beta.1-1", "1.1.0-1"] {
            add(&repo, &[file(&built, "example", version, "x86_64")]).unwrap();
            let report = index(&repo, &pacman, &key).unwrap();
            // Only the new package is signed.
            assert_eq!(report.signed.len(), 1);
        }
        let arch = repo.join("x86_64");
        assert!(!arch.join("example-1.0.0-1-x86_64.pkg.tar.zst").exists());
        assert!(!arch.join("example-1.0.0-1-x86_64.pkg.tar.zst.sig").exists());
        assert!(
            arch.join("example-1.1.0beta.1-1-x86_64.pkg.tar.zst")
                .exists()
        );

        let db = database_entries(&arch.join("example.db"));
        assert_eq!(db.len(), 1);
        let desc = &db["example-1.1.0-1/desc"];
        let package_file = arch.join("example-1.1.0-1-x86_64.pkg.tar.zst");
        let (sha256, size) = files::digest(&package_file).unwrap();
        assert!(desc.starts_with(
            "%FILENAME%\nexample-1.1.0-1-x86_64.pkg.tar.zst\n\n%NAME%\nexample\n\n%BASE%\nexample\n\n\
             %VERSION%\n1.1.0-1\n\n%DESC%\nAn example\n\n"
        ));
        assert!(desc.contains(&format!(
            "%CSIZE%\n{size}\n\n%ISIZE%\n1024\n\n%SHA256SUM%\n{sha256}\n\n%PGPSIG%\n"
        )));
        assert!(
            desc.ends_with("%DEPENDS%\nglibc\ngcc-libs\n\n%OPTDEPENDS%\nxdg-utils: open links\n\n")
        );
        let signature = std::fs::read(arch.join("example-1.1.0-1-x86_64.pkg.tar.zst.sig")).unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode(&signature);
        assert!(desc.contains(&format!("%PGPSIG%\n{encoded}\n\n")));
        public
            .verify_detached(&std::fs::read(&package_file).unwrap(), &signature)
            .unwrap();

        let files_db = database_entries(&arch.join("example.files.tar.gz"));
        assert!(files_db["example-1.1.0-1/files"].starts_with("%FILES%\nopt/\nopt/example/\n"));
        for name in [
            "example.db",
            "example.db.tar.gz",
            "example.files",
            "example.files.tar.gz",
        ] {
            let bytes = std::fs::read(arch.join(name)).unwrap();
            public
                .verify_detached(
                    &bytes,
                    &std::fs::read(arch.join(format!("{name}.sig"))).unwrap(),
                )
                .unwrap();
        }
        assert_eq!(
            std::fs::read(arch.join("example.db")).unwrap(),
            std::fs::read(arch.join("example.db.tar.gz")).unwrap()
        );
        let aarch64 = database_entries(&repo.join("aarch64/example.db"));
        assert!(aarch64.contains_key("example-1.0.0-1/desc"));
        assert!(repo.join("key.asc").is_file());

        // A changed package of a published version is refused.
        let changed = built.join("example-1.1.0-1-x86_64.pkg.tar.zst");
        std::fs::write(
            &changed,
            described("example", "1.1.0-1", "x86_64", "Changed"),
        )
        .unwrap();
        assert!(add(&repo, &[changed]).is_err());
    }

    #[test]
    fn info_lines_read() {
        let info = Package::parse_info("# comment\npkgname = a\ndepend = b=1\n").unwrap();
        assert_eq!(info[1], ("depend".to_owned(), "b=1".to_owned()));
        assert!(Package::parse_info("nonsense").is_err());
    }
}
