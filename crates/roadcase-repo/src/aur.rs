//! An AUR `-bin` package: a `PKGBUILD` and its `.SRCINFO` that repackage the
//! pacman packages a repository publishes, one per architecture.
//!
//! makepkg downloads the package for its architecture from
//! `<url>/<arch>/<file>` (the layout of [`crate::pacman`]), checks its
//! SHA-256, and unpacks its files into the new package, which provides and
//! conflicts with the original name. Publishing it is a `git push` of the
//! two files to `ssh://aur@aur.archlinux.org/<pkgname>.git`, which stays the
//! release's own step.

use crate::pacman::{Package, read_package};
use crate::{Error, Result, files};
use std::path::PathBuf;

/// What the `PKGBUILD` says beyond the packages.
#[derive(Debug, Clone, Default)]
pub struct Aur {
    /// The pacman repository's URL: the folder that holds `<arch>/`.
    pub url: String,
    /// The `# Maintainer:` line, such as `Example <packages@example.com>`.
    pub maintainer: Option<String>,
    /// The AUR package's name: `<pkgname>-bin` when absent.
    pub pkgname: Option<String>,
}

/// A `PKGBUILD` and its `.SRCINFO`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generated {
    /// The AUR package's name.
    pub pkgname: String,
    /// `PKGBUILD`.
    pub pkgbuild: String,
    /// `.SRCINFO`, as `makepkg --printsrcinfo` writes it.
    pub srcinfo: String,
}

struct Source {
    arch: String,
    file: String,
    url: String,
    sha256: String,
}

/// Writes the `PKGBUILD` and `.SRCINFO` for `packages`: one version of one
/// package, for one architecture each.
///
/// # Errors
///
/// [`Error::Invalid`] for no packages, packages of different names or
/// versions, or two of one architecture; [`Error::Package`]; [`Error::Io`].
pub fn generate(packages: &[PathBuf], aur: &Aur) -> Result<Generated> {
    let mut read: Vec<(Package, Source)> = Vec::new();
    for path in packages {
        let package = read_package(path)?;
        let file = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        files::check_name("the package file", &file)?;
        let arch = package.arch()?.to_owned();
        let (sha256, _) = files::digest(path)?;
        let url = format!("{}/{arch}/{file}", aur.url.trim_end_matches('/'));
        read.push((
            package,
            Source {
                arch,
                file,
                url,
                sha256,
            },
        ));
    }
    let Some((first, _)) = read.first() else {
        return Err(Error::Invalid("give the packages of one release".into()));
    };
    let name = first.name()?.to_owned();
    let full_version = first.version()?.to_owned();
    for (package, source) in &read {
        if package.name()? != name || package.version()? != full_version {
            return Err(Error::Invalid(format!(
                "{} is {} {}, not {name} {full_version}: give the packages of one release",
                source.file,
                package.name()?,
                package.version()?
            )));
        }
    }
    read.sort_by(|a, b| a.1.arch.cmp(&b.1.arch));
    if read.windows(2).any(|pair| pair[0].1.arch == pair[1].1.arch) {
        return Err(Error::Invalid("give one package per architecture".into()));
    }
    if read.iter().any(|(_, source)| source.arch == "any") && read.len() > 1 {
        return Err(Error::Invalid(
            "a package for any architecture is the only one".into(),
        ));
    }
    let (epoch, rest) = match full_version.split_once(':') {
        Some((epoch, rest)) => (Some(epoch.to_owned()), rest.to_owned()),
        None => (None, full_version.clone()),
    };
    let (pkgver, pkgrel) = rest
        .rsplit_once('-')
        .map(|(version, release)| (version.to_owned(), release.to_owned()))
        .ok_or_else(|| Error::Package(format!("{full_version} has no release (-1)")))?;
    let pkgname = aur.pkgname.clone().unwrap_or_else(|| format!("{name}-bin"));
    files::check_name("the AUR package", &pkgname)?;

    let first = &read[0].0;
    let pkgdesc = first.get("pkgdesc").unwrap_or_default().to_owned();
    let url = first.get("url").unwrap_or_default().to_owned();
    let arches: Vec<&str> = read
        .iter()
        .map(|(_, source)| source.arch.as_str())
        .collect();
    let licenses = first.all("license");
    let depends = first.all("depend");
    let optdepends = first.all("optdepend");
    let provides = format!("{name}={rest}", rest = pkgver);
    let noextract: Vec<&str> = read
        .iter()
        .map(|(_, source)| source.file.as_str())
        .collect();
    let options = ["!strip", "!debug"];

    let mut pkgbuild = String::new();
    if let Some(maintainer) = &aur.maintainer {
        pkgbuild.push_str(&format!("# Maintainer: {maintainer}\n"));
    }
    pkgbuild.push_str(&format!(
        "# Repackages {name} {full_version}, as published at {}.\n\n",
        aur.url
    ));
    let mut line = |key: &str, value: String| pkgbuild.push_str(&format!("{key}={value}\n"));
    line("pkgname", quote(&pkgname));
    line("pkgver", quote(&pkgver));
    line("pkgrel", quote(&pkgrel));
    if let Some(epoch) = &epoch {
        line("epoch", quote(epoch));
    }
    line("pkgdesc", quote(&pkgdesc));
    line("arch", array(&arches));
    if !url.is_empty() {
        line("url", quote(&url));
    }
    line("license", array(&licenses));
    if !depends.is_empty() {
        line("depends", array(&depends));
    }
    if !optdepends.is_empty() {
        line("optdepends", array(&optdepends));
    }
    line("provides", array(&[provides.as_str()]));
    line("conflicts", array(&[name.as_str()]));
    line("options", array(&options));
    line("noextract", array(&noextract));
    for (_, source) in &read {
        let entry = format!("{}::{}", source.file, source.url);
        line(&format!("source_{}", source.arch), array(&[entry.as_str()]));
        line(
            &format!("sha256sums_{}", source.arch),
            array(&[source.sha256.as_str()]),
        );
    }
    pkgbuild.push_str("\npackage() {\n  local file\n  case \"$CARCH\" in\n");
    for (_, source) in &read {
        pkgbuild.push_str(&format!(
            "    {}) file={} ;;\n",
            source.arch,
            quote(&source.file)
        ));
    }
    pkgbuild.push_str(
        "  esac\n  bsdtar -xpf \"$srcdir/$file\" -C \"$pkgdir\" \\\n    \
         --exclude .PKGINFO --exclude .MTREE --exclude .BUILDINFO --exclude .INSTALL --exclude .CHANGELOG\n}\n",
    );

    // As makepkg --printsrcinfo orders it.
    let mut srcinfo = format!("pkgbase = {pkgname}\n");
    let mut attr = |key: &str, values: &[&str]| {
        for value in values {
            srcinfo.push_str(&format!("\t{key} = {value}\n"));
        }
    };
    attr("pkgdesc", &[&pkgdesc]);
    attr("pkgver", &[&pkgver]);
    attr("pkgrel", &[&pkgrel]);
    if let Some(epoch) = &epoch {
        attr("epoch", &[epoch]);
    }
    if !url.is_empty() {
        attr("url", &[&url]);
    }
    attr("arch", &arches);
    attr("license", &licenses);
    attr("depends", &depends);
    attr("optdepends", &optdepends);
    attr("provides", &[&provides]);
    attr("conflicts", &[&name]);
    attr("noextract", &noextract);
    attr("options", &options);
    for (_, source) in &read {
        let entry = format!("{}::{}", source.file, source.url);
        attr(&format!("source_{}", source.arch), &[&entry]);
        attr(&format!("sha256sums_{}", source.arch), &[&source.sha256]);
    }
    srcinfo.push_str(&format!("\npkgname = {pkgname}\n"));

    Ok(Generated {
        pkgname,
        pkgbuild,
        srcinfo,
    })
}

/// A word for bash, in single quotes.
pub fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn array(items: &[&str]) -> String {
    format!(
        "({})",
        items
            .iter()
            .map(|item| quote(item))
            .collect::<Vec<_>>()
            .join(" ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pacman::tests::file;

    #[test]
    fn a_bin_package_names_each_architecture_with_its_hash() {
        let dir = tempfile::tempdir().unwrap();
        let x86 = file(dir.path(), "example", "2.0.0-1", "x86_64");
        let arm = file(dir.path(), "example", "2.0.0-1", "aarch64");
        let aur = Aur {
            url: "https://packages.example.com/arch/".into(),
            maintainer: Some("Example <packages@example.com>".into()),
            pkgname: None,
        };
        let generated = generate(&[x86.clone(), arm.clone()], &aur).unwrap();
        assert_eq!(generated.pkgname, "example-bin");
        let (x86_sha, _) = files::digest(&x86).unwrap();
        let pkgbuild = &generated.pkgbuild;
        assert!(pkgbuild.starts_with("# Maintainer: Example <packages@example.com>\n"));
        for expected in [
            "pkgname='example-bin'\npkgver='2.0.0'\npkgrel='1'\npkgdesc='An example'\n",
            "arch=('aarch64' 'x86_64')\nurl='https://example.com'\nlicense=('MIT')\n",
            "depends=('glibc' 'gcc-libs')\noptdepends=('xdg-utils: open links')\n",
            "provides=('example=2.0.0')\nconflicts=('example')\noptions=('!strip' '!debug')\n",
            "source_x86_64=('example-2.0.0-1-x86_64.pkg.tar.zst::https://packages.example.com/arch/x86_64/example-2.0.0-1-x86_64.pkg.tar.zst')\n",
            "    x86_64) file='example-2.0.0-1-x86_64.pkg.tar.zst' ;;\n",
        ] {
            assert!(pkgbuild.contains(expected), "{expected}\nin\n{pkgbuild}");
        }
        assert!(pkgbuild.contains(&format!("sha256sums_x86_64=('{x86_sha}')\n")));
        let srcinfo = &generated.srcinfo;
        assert!(
            srcinfo
                .starts_with("pkgbase = example-bin\n\tpkgdesc = An example\n\tpkgver = 2.0.0\n")
        );
        assert!(srcinfo.contains(&format!("\tsha256sums_x86_64 = {x86_sha}\n")));
        assert!(srcinfo.ends_with("\npkgname = example-bin\n"));

        let other = file(dir.path(), "example", "2.1.0-1", "x86_64");
        assert!(generate(&[arm.clone(), other], &aur).is_err());
        assert!(generate(&[x86.clone(), x86], &aur).is_err());
        assert!(generate(&[], &aur).is_err());
    }

    #[test]
    fn words_are_quoted_for_bash() {
        assert_eq!(quote("it's"), "'it'\\''s'");
    }
}
