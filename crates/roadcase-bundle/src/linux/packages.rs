//! System packages made from the AppDir: a `.deb` with `dpkg-deb`, and a
//! pacman package with `makepkg`. Both install the same tree:
//!
//! ```text
//! /opt/<package>/...                  the AppDir's usr/ (bin/, lib/, share/...)
//! /opt/<package>/.roadcase-install    deb or pacman
//! /usr/bin/<executable>               a link to /opt/<package>/bin/<executable>
//! /usr/share/applications/<id>.desktop
//! /usr/share/icons/hicolor/..., /usr/share/mime/packages/<id>.xml, /usr/share/metainfo/...
//! ```
//!
//! Neither has maintainer scripts or install hooks: dpkg's triggers and
//! pacman's hooks refresh the desktop's caches, and the package manager is
//! what updates the application.

use super::{AppDir, executable_name};
use crate::error::io;
use crate::{Error, Project, Result};
use roadcase_update::{INSTALL_MARKER, PackageManager};
use std::path::{Path, PathBuf};

/// The `.deb`'s package name: `[linux.deb] package`, or the executable's name.
///
/// # Errors
///
/// [`Error::Config`] without `[linux.deb]`; [`Error::Value`].
pub fn deb_package(project: &Project) -> Result<String> {
    let deb = deb_config(project)?;
    match &deb.package {
        Some(name) => project.context.fill(name),
        None => executable_name(project),
    }
}

/// The pacman package's name: `[linux.pacman] pkgname`, or the executable's
/// name.
///
/// # Errors
///
/// [`Error::Config`] without `[linux.pacman]`; [`Error::Value`].
pub fn pacman_package(project: &Project) -> Result<String> {
    let pacman = pacman_config(project)?;
    match &pacman.pkgname {
        Some(name) => project.context.fill(name),
        None => executable_name(project),
    }
}

fn deb_config(project: &Project) -> Result<&crate::config::Deb> {
    project
        .linux()?
        .deb
        .as_ref()
        .ok_or_else(|| Error::Config("the config has no [linux.deb]".into()))
}

fn pacman_config(project: &Project) -> Result<&crate::config::Pacman> {
    project
        .linux()?
        .pacman
        .as_ref()
        .ok_or_else(|| Error::Config("the config has no [linux.pacman]".into()))
}

/// Debian's name of an architecture: `amd64`, `arm64`.
///
/// # Errors
///
/// [`Error::Config`] for one roadcase does not package.
pub fn deb_arch(arch: &str) -> Result<&'static str> {
    match arch {
        "x86_64" => Ok("amd64"),
        "aarch64" => Ok("arm64"),
        other => Err(Error::Config(format!("{other} is not packaged for Debian"))),
    }
}

/// Arch Linux's name of an architecture: `x86_64`, `aarch64`.
///
/// # Errors
///
/// [`Error::Config`] for one roadcase does not package.
pub fn pacman_arch(arch: &str) -> Result<&'static str> {
    match arch {
        "x86_64" => Ok("x86_64"),
        "aarch64" => Ok("aarch64"),
        other => Err(Error::Config(format!(
            "{other} is not packaged for Arch Linux"
        ))),
    }
}

/// The Debian version of a semantic version: a pre-release sorts before its
/// release (`1.2.0-beta.1` is `1.2.0~beta.1`), then `-<revision>`.
pub fn deb_version(version: &str, revision: &str) -> String {
    let (core, build) = match version.split_once('+') {
        Some((core, build)) => (core, Some(build)),
        None => (version, None),
    };
    let mut upstream = match core.split_once('-') {
        Some((release, pre)) => format!("{release}~{pre}"),
        None => core.to_owned(),
    };
    if let Some(build) = build {
        upstream.push('+');
        upstream.push_str(build);
    }
    format!("{upstream}-{revision}")
}

/// pacman's `pkgver` of a semantic version: a pre-release follows its release
/// with no separator, which sorts it first (`1.2.0-beta.1` is
/// `1.2.0beta.1`); hyphens inside it become `_`, and build metadata is
/// dropped, since a `pkgver` holds no `-` and pacman would order what follows
/// a `+`.
pub fn pacman_version(version: &str) -> String {
    let core = version.split('+').next().unwrap_or_default();
    match core.split_once('-') {
        Some((release, pre)) => format!("{release}{}", pre.replace('-', "_")),
        None => core.to_owned(),
    }
}

/// The summary line both packages describe the application with:
/// `linux.comment`, or the name.
fn summary(project: &Project) -> Result<String> {
    let linux = project.linux()?;
    match &linux.comment {
        Some(comment) => project.context.fill(comment),
        None => Ok(project.context.name().to_owned()),
    }
}

/// Fills `root` with what a package installs, under `/opt/<package>`, with
/// the marker saying which package manager installed it.
///
/// # Errors
///
/// [`Error::Value`]; [`Error::Io`].
pub fn package_tree(
    project: &Project,
    app: &AppDir,
    root: &Path,
    package: &str,
    manager: PackageManager,
) -> Result<()> {
    let id = project.context.id().to_owned();
    if root.exists() {
        std::fs::remove_dir_all(root).map_err(io(root))?;
    }
    let prefix = root.join("opt").join(package);
    let usr = app.path.join("usr");
    let system = ["applications", "icons", "mime", "metainfo"];
    for entry in std::fs::read_dir(&usr).map_err(io(&usr))? {
        let entry = entry.map_err(io(&usr))?;
        let name = entry.file_name();
        if name == "share" {
            let share = entry.path();
            for item in std::fs::read_dir(&share).map_err(io(&share))? {
                let item = item.map_err(io(&share))?;
                let item_name = item.file_name();
                let to = if system.iter().any(|ours| item_name == *ours) {
                    if item_name == "applications" {
                        continue;
                    }
                    root.join("usr/share").join(&item_name)
                } else {
                    prefix.join("share").join(&item_name)
                };
                copy_tree(&item.path(), &to)?;
            }
        } else {
            copy_tree(&entry.path(), &prefix.join(&name))?;
        }
    }
    write(
        &root
            .join("usr/share/applications")
            .join(format!("{id}.desktop")),
        super::package_desktop_entry(project)?.as_bytes(),
    )?;
    write(
        &prefix.join(INSTALL_MARKER),
        format!("{}\n", manager.marker()).as_bytes(),
    )?;
    let executable = executable_name(project)?;
    let link = root.join("usr/bin").join(&executable);
    std::fs::create_dir_all(root.join("usr/bin")).map_err(io(root.join("usr/bin")))?;
    symlink(&format!("/opt/{package}/bin/{executable}"), &link)?;
    normalize(root)
}

/// Copies a file, a link or a folder, keeping links as links and whether a
/// file is executable.
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    let kind = std::fs::symlink_metadata(from).map_err(io(from))?;
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    if kind.file_type().is_symlink() {
        let target = std::fs::read_link(from).map_err(io(from))?;
        return symlink(&target.to_string_lossy(), to);
    }
    if kind.is_dir() {
        std::fs::create_dir_all(to).map_err(io(to))?;
        for entry in std::fs::read_dir(from).map_err(io(from))? {
            let entry = entry.map_err(io(from))?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        return Ok(());
    }
    std::fs::copy(from, to).map(|_| ()).map_err(io(from))
}

#[cfg(unix)]
fn symlink(target: &str, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link).map_err(io(link))
}

#[cfg(not(unix))]
fn symlink(_: &str, link: &Path) -> Result<()> {
    Err(Error::Config(format!(
        "{}: Linux packages are made on Linux",
        link.display()
    )))
}

/// Folders 755, files 644 or 755 when executable: what dpkg and pacman
/// expect of a package, whatever the umask was.
#[cfg(unix)]
fn normalize(root: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::symlink_metadata(root).map_err(io(root))?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    let mode = if metadata.is_dir() || metadata.permissions().mode() & 0o111 != 0 {
        0o755
    } else {
        0o644
    };
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(mode)).map_err(io(root))?;
    if metadata.is_dir() {
        for entry in std::fs::read_dir(root).map_err(io(root))? {
            normalize(&entry.map_err(io(root))?.path())?;
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn normalize(_: &Path) -> Result<()> {
    Ok(())
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    std::fs::write(path, bytes).map_err(io(path))
}

/// The `.deb`'s control file.
///
/// # Errors
///
/// [`Error::Config`] without `[linux.deb]`, or without a maintainer;
/// [`Error::Value`].
pub fn deb_control(project: &Project, depends: &[String], installed_kib: u64) -> Result<String> {
    let deb = deb_config(project)?;
    let context = &project.context;
    let maintainer = deb
        .maintainer
        .clone()
        .or_else(|| project.config.app.publisher.clone())
        .ok_or_else(|| {
            Error::Config(
                "linux.deb.maintainer (or app.publisher) names who maintains the package".into(),
            )
        })?;
    let mut text = String::new();
    let mut field = |key: &str, value: &str| {
        if !value.is_empty() {
            text.push_str(&format!("{key}: {value}\n"));
        }
    };
    field("Package", &deb_package(project)?);
    field(
        "Version",
        &deb_version(context.version(), &context.fill(&deb.revision)?),
    );
    field(
        "Architecture",
        deb_arch(context.get("arch").unwrap_or_default())?,
    );
    field("Maintainer", &context.fill(&maintainer)?);
    field("Installed-Size", &installed_kib.to_string());
    field("Depends", &depends.join(", "));
    let recommends = fill_all(project, &deb.recommends)?;
    field("Recommends", &recommends.join(", "));
    field("Section", &context.fill(&deb.section)?);
    field("Priority", &context.fill(&deb.priority)?);
    if let Some(homepage) = &deb.homepage {
        field("Homepage", &context.fill(homepage)?);
    }
    let mut description = summary(project)?.replace('\n', " ");
    if let Some(long) = &deb.description {
        for line in context.fill(long)?.trim().lines() {
            description.push_str("\n ");
            description.push_str(if line.trim().is_empty() {
                "."
            } else {
                line.trim_end()
            });
        }
    }
    field("Description", &description);
    Ok(text)
}

fn fill_all(project: &Project, items: &[String]) -> Result<Vec<String>> {
    items
        .iter()
        .map(|item| project.context.fill(item))
        .collect()
}

/// The deb822 apt source the `.deb` installs, and where its key goes.
///
/// # Errors
///
/// [`Error::Config`] without `[linux.deb]`; [`Error::Value`].
pub fn apt_source(project: &Project, keyring: &str) -> Result<Option<String>> {
    let deb = deb_config(project)?;
    let Some(source) = &deb.apt_source else {
        return Ok(None);
    };
    let context = &project.context;
    Ok(Some(format!(
        "Types: deb\nURIs: {}\nSuites: {}\nComponents: {}\nSigned-By: {keyring}\n",
        context.fill(&source.url)?,
        context.fill(&source.suite)?,
        fill_all(project, &source.components)?.join(" ")
    )))
}

/// The `Depends` list: the declared ones, then those of `found` (from
/// `dpkg-shlibdeps`) for packages not declared.
pub fn merge_depends(declared: &[String], found: &[String]) -> Vec<String> {
    let name = |item: &str| {
        item.split([' ', '(', ':'])
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned()
    };
    let mut names: Vec<String> = declared.iter().map(|item| name(item)).collect();
    let mut merged = declared.to_vec();
    for item in found {
        // An alternative (a | b) is kept whole.
        let first = name(item);
        if !names.contains(&first) {
            names.push(first);
            merged.push(item.clone());
        }
    }
    merged
}

/// Makes the `.deb` from the AppDir, with `dpkg-deb --root-owner-group`, into
/// `out`.
///
/// # Errors
///
/// [`Error::Config`] without `[linux.deb]`; [`Error::Missing`] for the
/// apt source's key; [`Error::Command`] when `dpkg-deb` is missing or fails;
/// [`Error::Value`]; [`Error::Io`].
pub fn deb(project: &Project, app: &AppDir, out: &Path) -> Result<PathBuf> {
    let deb = deb_config(project)?;
    let context = &project.context;
    let package = deb_package(project)?;
    let work = out.join(format!("{}.deb.d", context.name()));
    let root = work.join("root");
    package_tree(project, app, &root, &package, PackageManager::Apt)?;

    let mut conffiles = String::new();
    if let Some(source) = &deb.apt_source {
        let from = context.path(&source.key)?;
        let key = std::fs::read(&from).map_err(|_| {
            Error::Missing(format!(
                "linux.deb.apt_source.key: {} is not there",
                from.display()
            ))
        })?;
        let extension = if key.trim_ascii_start().starts_with(b"-----BEGIN") {
            "asc"
        } else {
            "gpg"
        };
        let keyring = format!("/usr/share/keyrings/{package}-archive-keyring.{extension}");
        write(&root.join(&keyring[1..]), &key)?;
        let sources = format!("/etc/apt/sources.list.d/{package}.sources");
        let text = apt_source(project, &keyring)?.unwrap_or_default();
        write(&root.join(&sources[1..]), text.as_bytes())?;
        conffiles.push_str(&sources);
        conffiles.push('\n');
    }

    let mut depends = fill_all(project, &deb.depends)?;
    if deb.shlibdeps && on_path("dpkg-shlibdeps") {
        let found = shlibdeps(&work, &root, &package)?;
        depends = merge_depends(&depends, &found);
    }
    let control = deb_control(project, &depends, installed_kib(&root)?)?;
    let debian = root.join("DEBIAN");
    write(&debian.join("control"), control.as_bytes())?;
    if !conffiles.is_empty() {
        write(&debian.join("conffiles"), conffiles.as_bytes())?;
    }
    normalize(&debian)?;

    std::fs::create_dir_all(out).map_err(io(out))?;
    let name = match &deb.name {
        Some(name) => context.fill(name)?,
        None => format!(
            "{package}_{}_{}.deb",
            deb_version(context.version(), &context.fill(&deb.revision)?),
            deb_arch(context.get("arch").unwrap_or_default())?
        ),
    };
    let target = out.join(name);
    if target.exists() {
        std::fs::remove_file(&target).map_err(io(&target))?;
    }
    let output = std::process::Command::new("dpkg-deb")
        .args(["--root-owner-group", "--build"])
        .arg(&root)
        .arg(&target)
        .output()
        .map_err(|error| {
            Error::Command(format!(
                "dpkg-deb: {error}: a .deb is made on Linux with dpkg-deb (the dpkg package)"
            ))
        })?;
    if !output.status.success() {
        return Err(Error::Command(format!(
            "dpkg-deb failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    std::fs::remove_dir_all(&work).map_err(io(&work))?;
    Ok(target)
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|folder| folder.join(program).is_file())
    })
}

/// Whether a file is an ELF executable or library.
fn is_elf(path: &Path) -> bool {
    use std::io::Read;
    let mut magic = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .is_ok()
        && magic == *b"\x7fELF"
}

/// The packages the package's ELF files need, from `dpkg-shlibdeps`.
fn shlibdeps(work: &Path, root: &Path, package: &str) -> Result<Vec<String>> {
    let prefix = root.join("opt").join(package);
    let mut elves = Vec::new();
    let mut pending = vec![prefix.clone()];
    while let Some(folder) = pending.pop() {
        for entry in std::fs::read_dir(&folder).map_err(io(&folder))? {
            let entry = entry.map_err(io(&folder))?;
            let kind = entry.file_type().map_err(io(entry.path()))?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() && is_elf(&entry.path()) {
                elves.push(entry.path());
            }
        }
    }
    if elves.is_empty() {
        return Ok(Vec::new());
    }
    elves.sort();
    // dpkg-shlibdeps reads debian/control from where it runs.
    let scratch = work.join("shlibdeps");
    write(
        &scratch.join("debian/control"),
        format!("Source: {package}\n\nPackage: {package}\nArchitecture: any\n").as_bytes(),
    )?;
    let mut command = std::process::Command::new("dpkg-shlibdeps");
    command
        .current_dir(&scratch)
        .args(["-O", "--ignore-missing-info"]);
    let lib = prefix.join("lib");
    if lib.is_dir() {
        command.arg(format!("-l{}", lib.display()));
    }
    for elf in &elves {
        command.arg(format!("-e{}", elf.display()));
    }
    let output = command
        .output()
        .map_err(|error| Error::Command(format!("dpkg-shlibdeps: {error}")))?;
    std::fs::remove_dir_all(&scratch).map_err(io(&scratch))?;
    if !output.status.success() {
        return Err(Error::Command(format!(
            "dpkg-shlibdeps failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(parse_shlibdeps(&String::from_utf8_lossy(&output.stdout)))
}

/// `shlibs:Depends=libc6 (>= 2.34), libgcc-s1 (>= 4.2)` as a list.
pub fn parse_shlibdeps(output: &str) -> Vec<String> {
    output
        .lines()
        .find_map(|line| line.strip_prefix("shlibs:Depends="))
        .map(|list| {
            list.split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// What the tree takes once installed, in KiB, as `Installed-Size` has it.
fn installed_kib(root: &Path) -> Result<u64> {
    let mut total = 0;
    let mut pending = vec![root.to_path_buf()];
    while let Some(folder) = pending.pop() {
        for entry in std::fs::read_dir(&folder).map_err(io(&folder))? {
            let entry = entry.map_err(io(&folder))?;
            let metadata = std::fs::symlink_metadata(entry.path()).map_err(io(entry.path()))?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                total += metadata.len().div_ceil(1024);
            } else {
                total += 1;
            }
        }
    }
    Ok(total.max(1))
}

/// The `PKGBUILD` that packages `tree` as it is.
///
/// # Errors
///
/// [`Error::Config`] without `[linux.pacman]`; [`Error::Value`].
pub fn pkgbuild(project: &Project, tree: &Path) -> Result<String> {
    let pacman = pacman_config(project)?;
    let context = &project.context;
    let description = match &pacman.description {
        Some(description) => context.fill(description)?,
        None => summary(project)?,
    };
    let mut text =
        String::from("# Written by roadcase: it packages the files already in the tree below.\n");
    let mut line = |key: &str, value: String| text.push_str(&format!("{key}={value}\n"));
    line("pkgname", quote(&pacman_package(project)?));
    line("pkgver", quote(&pacman_version(context.version())));
    line("pkgrel", quote(&context.fill(&pacman.pkgrel)?));
    line("pkgdesc", quote(&description.replace('\n', " ")));
    line(
        "arch",
        array(&[pacman_arch(context.get("arch").unwrap_or_default())?.to_owned()]),
    );
    if let Some(url) = &pacman.url {
        line("url", quote(&context.fill(url)?));
    }
    line("license", array(&fill_all(project, &pacman.license)?));
    line("depends", array(&fill_all(project, &pacman.depends)?));
    line("optdepends", array(&fill_all(project, &pacman.optdepends)?));
    // Symbols were split and executables stripped with the AppDir.
    line("options", array(&["!strip".into(), "!debug".into()]));
    text.push_str(&format!(
        "\npackage() {{\n  cp -a {}/. \"$pkgdir\"/\n}}\n",
        quote(&tree.to_string_lossy())
    ));
    Ok(text)
}

/// A word for bash, in single quotes.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn array(items: &[String]) -> String {
    format!(
        "({})",
        items
            .iter()
            .map(|item| quote(item))
            .collect::<Vec<_>>()
            .join(" ")
    )
}

/// Makes the pacman package from the AppDir with `makepkg`, into `out`: on
/// Arch Linux, as a person other than root. Nothing is downloaded: the
/// `PKGBUILD` has no sources, and `--nodeps` builds without installing the
/// dependencies it declares.
///
/// # Errors
///
/// [`Error::Config`] without `[linux.pacman]`; [`Error::Command`] when
/// `makepkg` is missing or fails; [`Error::Value`]; [`Error::Io`].
pub fn pacman(project: &Project, app: &AppDir, out: &Path) -> Result<PathBuf> {
    let pacman = pacman_config(project)?;
    let context = &project.context;
    let package = pacman_package(project)?;
    std::fs::create_dir_all(out).map_err(io(out))?;
    let out = out.canonicalize().map_err(io(out))?;
    let work = out.join(format!("{}.pacman.d", context.name()));
    let tree = work.join("tree");
    package_tree(project, app, &tree, &package, PackageManager::Pacman)?;
    write(&work.join("PKGBUILD"), pkgbuild(project, &tree)?.as_bytes())?;

    let tool = context.resolve(&pacman.makepkg, "linux.pacman.makepkg")?;
    let name = format!(
        "{package}-{}-{}-{}.pkg.tar.zst",
        pacman_version(context.version()),
        context.fill(&pacman.pkgrel)?,
        pacman_arch(context.get("arch").unwrap_or_default())?
    );
    let target = out.join(&name);
    let output = std::process::Command::new(&tool)
        .args(["--force", "--nodeps", "--noconfirm", "--noprogressbar"])
        .current_dir(&work)
        .env("PKGDEST", &out)
        .env("BUILDDIR", work.join("build"))
        .env("SRCDEST", work.join("sources"))
        .env("PKGEXT", ".pkg.tar.zst")
        .output()
        .map_err(|error| {
            Error::Command(format!(
                "{tool}: {error}: a pacman package is made on Arch Linux with makepkg (in CI, in an \
                 Arch Linux container on a Linux runner of the architecture)"
            ))
        })?;
    if !output.status.success() {
        return Err(Error::Command(format!(
            "{tool} failed: {}{}",
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    if !target.is_file() {
        return Err(Error::Command(format!(
            "{tool} succeeded and wrote no {}",
            target.display()
        )));
    }
    std::fs::remove_dir_all(&work).map_err(io(&work))?;
    Ok(target)
}
