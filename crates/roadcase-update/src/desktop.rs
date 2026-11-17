//! Desktop integration for an application that runs as an AppImage.
//!
//! An AppImage has no installer, so nothing tells the desktop about it: its
//! menu entry, its URL schemes and its document types exist only once a
//! desktop entry and a shared-mime-info file are in `~/.local/share`. The
//! application does that itself, at startup:
//!
//! ```no_run
//! # fn main() -> Result<(), roadcase_update::Error> {
//! match roadcase_update::desktop::install() {
//!     Ok(_changed) => {}
//!     // Started from a build folder or a package: nothing to do.
//!     Err(roadcase_update::Error::NotAppImage) => {}
//!     Err(error) => eprintln!("desktop integration: {error}"),
//! }
//! # Ok(())
//! # }
//! ```
//!
//! [`install`] copies, from the AppImage's own AppDir (`$APPDIR`, which the
//! packager filled):
//!
//! - `<id>.desktop` into `applications/`, its `Exec` pointing at the
//!   AppImage's file (`$APPIMAGE`) and a `TryExec` that hides the entry
//!   once that file is gone;
//! - `usr/share/mime/packages/<id>.xml` into `mime/packages/`;
//! - `usr/share/icons/hicolor/*/apps/<id>.*` into `icons/hicolor/`;
//!
//! under `$XDG_DATA_HOME`, or `~/.local/share`, then runs
//! `update-desktop-database` and `update-mime-database` when they are
//! installed and something changed. It is idempotent and cheap when nothing
//! changed, and follows the AppImage when it is moved or renamed, since the
//! next start writes the new path. An update that gives the AppImage a new
//! name (one named after its version) is followed the same way: the new
//! release starts with its own `$APPIMAGE`, and its first call to [`install`]
//! points the entry at it. Shortcuts the person made by hand to the old path
//! are not the application's to follow.
//!
//! [`remove`] takes it all away again, for a "remove from the menu" action.

use crate::{Error, Result};
use std::path::{Path, PathBuf};

/// Where an AppImage integrates, and what it integrates from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Integration {
    /// The application's identifier: the AppDir's desktop entry is
    /// `<id>.desktop`, and every file installed is named after it.
    pub id: String,
    /// The AppImage's file, which the desktop entry starts.
    pub appimage: PathBuf,
    /// The mounted AppDir the files come from.
    pub appdir: PathBuf,
    /// The user's data folder: `$XDG_DATA_HOME`, or `~/.local/share`.
    pub data_home: PathBuf,
    /// Whether the desktop's caches are refreshed after a change.
    pub refresh: bool,
}

/// Integrates the running AppImage. Answers whether anything changed.
///
/// # Errors
///
/// As [`Integration::from_env`] and [`Integration::install`].
pub fn install() -> Result<bool> {
    Integration::from_env()?.install()
}

/// Removes the running AppImage's integration. Answers whether anything was
/// there.
///
/// # Errors
///
/// As [`Integration::from_env`] and [`Integration::remove`].
pub fn remove() -> Result<bool> {
    Integration::from_env()?.remove()
}

impl Integration {
    /// The running AppImage's integration, from `$APPIMAGE`, `$APPDIR` and
    /// the one desktop entry at the AppDir's top.
    ///
    /// # Errors
    ///
    /// [`Error::NotAppImage`] when `$APPIMAGE` or `$APPDIR` is not set;
    /// [`Error::Config`] when the AppDir has no single desktop entry, or
    /// neither `$XDG_DATA_HOME` nor `$HOME` is set.
    pub fn from_env() -> Result<Self> {
        let variable = |name: &str| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        let (Some(appimage), Some(appdir)) = (variable("APPIMAGE"), variable("APPDIR")) else {
            return Err(Error::NotAppImage);
        };
        let data_home = match variable("XDG_DATA_HOME").filter(|path| path.is_absolute()) {
            Some(path) => path,
            None => variable("HOME")
                .ok_or_else(|| Error::Config("neither $XDG_DATA_HOME nor $HOME is set".into()))?
                .join(".local/share"),
        };
        Ok(Self {
            id: id_of(&appdir)?,
            appimage,
            appdir,
            data_home,
            refresh: true,
        })
    }

    fn installed_entry(&self) -> PathBuf {
        self.data_home
            .join("applications")
            .join(format!("{}.desktop", self.id))
    }

    fn installed_mime(&self) -> PathBuf {
        self.data_home
            .join("mime/packages")
            .join(format!("{}.xml", self.id))
    }

    /// Writes the desktop entry, MIME types and icons where the desktop
    /// finds them, leaving files that are already right alone. Answers
    /// whether anything changed.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] for an identifier that is not reverse DNS;
    /// [`Error::Io`], also when the AppDir has no `<id>.desktop`.
    pub fn install(&self) -> Result<bool> {
        check_id(&self.id)?;
        let entry = std::fs::read_to_string(self.appdir.join(format!("{}.desktop", self.id)))?;
        let entry = rewrite_entry(&entry, &self.appimage);
        let entry_changed = write_if_changed(&self.installed_entry(), entry.as_bytes())?;

        let source = self
            .appdir
            .join("usr/share/mime/packages")
            .join(format!("{}.xml", self.id));
        let mime_changed = if source.is_file() {
            write_if_changed(&self.installed_mime(), &std::fs::read(&source)?)?
        } else {
            remove_file(&self.installed_mime())?
        };

        let mut icons_changed = false;
        let theme = self.appdir.join("usr/share/icons/hicolor");
        let wanted = icons(&theme, &self.id);
        for relative in &wanted {
            let bytes = std::fs::read(theme.join(relative))?;
            let target = self.data_home.join("icons/hicolor").join(relative);
            icons_changed |= write_if_changed(&target, &bytes)?;
        }
        for relative in icons(&self.data_home.join("icons/hicolor"), &self.id) {
            if !wanted.contains(&relative) {
                icons_changed |= remove_file(&self.data_home.join("icons/hicolor").join(relative))?;
            }
        }

        self.refreshed(entry_changed, mime_changed);
        Ok(entry_changed || mime_changed || icons_changed)
    }

    /// Removes the desktop entry, MIME types and icons [`Integration::install`]
    /// wrote. Answers whether anything was there.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] for an identifier that is not reverse DNS;
    /// [`Error::Io`].
    pub fn remove(&self) -> Result<bool> {
        check_id(&self.id)?;
        let entry = remove_file(&self.installed_entry())?;
        let mime = remove_file(&self.installed_mime())?;
        let mut icons_removed = false;
        let theme = self.data_home.join("icons/hicolor");
        for relative in icons(&theme, &self.id) {
            icons_removed |= remove_file(&theme.join(relative))?;
        }
        self.refreshed(entry, mime);
        Ok(entry || mime || icons_removed)
    }

    /// Tells the desktop's caches, when their tools are installed. A missing
    /// or failing tool only means the desktop notices later.
    fn refreshed(&self, entry: bool, mime: bool) {
        if !self.refresh {
            return;
        }
        let run = |program: &str, arguments: &[&std::ffi::OsStr]| {
            let _ = std::process::Command::new(program)
                .args(arguments)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        };
        if entry {
            let folder = self.data_home.join("applications");
            run(
                "update-desktop-database",
                &["-q".as_ref(), folder.as_os_str()],
            );
        }
        if mime {
            let folder = self.data_home.join("mime");
            run("update-mime-database", &[folder.as_os_str()]);
        }
    }
}

/// The identifier of the one desktop entry at an AppDir's top.
fn id_of(appdir: &Path) -> Result<String> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(appdir)? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "desktop")
            && let Some(stem) = path.file_stem()
        {
            found.push(stem.to_string_lossy().into_owned());
        }
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        count => Err(Error::Config(format!(
            "{} holds {count} desktop entries, not one",
            appdir.display()
        ))),
    }
}

/// An identifier is reverse DNS, so it names files and nothing else.
fn check_id(id: &str) -> Result<()> {
    let parts: Vec<&str> = id.split('.').collect();
    let fine = parts.len() >= 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        });
    if fine {
        Ok(())
    } else {
        Err(Error::Config(format!(
            "{id:?} is not a reverse-DNS identifier"
        )))
    }
}

/// `<size>/apps/<id>.<extension>` under a hicolor theme folder.
fn icons(theme: &Path, id: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(sizes) = std::fs::read_dir(theme) else {
        return found;
    };
    for size in sizes.flatten() {
        for extension in ["png", "svg", "xpm"] {
            let name = format!("{id}.{extension}");
            if size.path().join("apps").join(&name).is_file() {
                found.push(PathBuf::from(size.file_name()).join("apps").join(name));
            }
        }
    }
    found.sort();
    found
}

/// Writes `bytes` unless the file already holds them. Answers whether it
/// wrote.
fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<bool> {
    if std::fs::read(path).is_ok_and(|found| found == bytes) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Written whole, then renamed: the desktop never reads half an entry.
    let temporary = path.with_extension("roadcase-partial");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path)?;
    Ok(true)
}

/// Removes a file. Answers whether it was there.
fn remove_file(path: &Path) -> Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// The desktop entry with every `Exec` starting `appimage` instead of the
/// AppDir's executable, and `TryExec` naming it.
pub fn rewrite_entry(entry: &str, appimage: &Path) -> String {
    let program = exec_quote(&appimage.to_string_lossy());
    let mut out = String::with_capacity(entry.len() + 128);
    let mut group = "";
    for line in entry.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            group = trimmed;
        }
        if trimmed.starts_with("TryExec=") || trimmed.starts_with("TryExec =") {
            continue;
        }
        if let Some(value) = trimmed
            .strip_prefix("Exec=")
            .or_else(|| trimmed.strip_prefix("Exec ="))
        {
            let (_, rest) = split_program(value.trim_start());
            out.push_str("Exec=");
            out.push_str(&escape(&program));
            out.push_str(rest);
            out.push('\n');
            if group == "[Desktop Entry]" {
                out.push_str("TryExec=");
                out.push_str(&escape(&appimage.to_string_lossy()));
                out.push('\n');
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The program of an `Exec` value, as written, and what follows it.
fn split_program(value: &str) -> (&str, &str) {
    if let Some(quoted) = value.strip_prefix('"') {
        let mut escaped = false;
        for (at, c) in quoted.char_indices() {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => return value.split_at(at + 2),
                _ => {}
            }
        }
        (value, "")
    } else {
        match value.find([' ', '\t']) {
            Some(at) => value.split_at(at),
            None => (value, ""),
        }
    }
}

/// A desktop entry string value: `\`, newlines, tabs and a leading space
/// escaped.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (index, c) in text.chars().enumerate() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            ' ' if index == 0 => out.push_str("\\s"),
            c => out.push(c),
        }
    }
    out
}

/// One `Exec` argument, quoted as the desktop entry specification asks:
/// a field code as it is, anything with a reserved character in double
/// quotes with `"`, `` ` ``, `$` and `\` escaped, and `%` doubled. The value
/// is then escaped as a string, with [`escape`].
pub fn exec_quote(argument: &str) -> String {
    const FIELD_CODES: &[&str] = &["%f", "%F", "%u", "%U", "%i", "%c", "%k"];
    if FIELD_CODES.contains(&argument) {
        return argument.to_owned();
    }
    let argument = argument.replace('%', "%%");
    let reserved = |c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '"' | '\''
                    | '\\'
                    | '>'
                    | '<'
                    | '~'
                    | '|'
                    | '&'
                    | ';'
                    | '$'
                    | '*'
                    | '?'
                    | '#'
                    | '('
                    | ')'
                    | '`'
            )
    };
    if !argument.is_empty() && !argument.chars().any(reserved) {
        return argument;
    }
    let mut quoted = String::from("\"");
    for c in argument.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted
}

// Desktop entries hold Unix paths; the tests write and compare them as such.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    const ENTRY: &str = "[Desktop Entry]\n\
        Type=Application\n\
        Name=Example Studio\n\
        Exec=example-studio --open %u\n\
        TryExec=example-studio\n\
        Icon=com.example.studio\n\
        MimeType=x-scheme-handler/examplestudio;application/x-exproj;\n\
        \n\
        [Desktop Action new]\n\
        Name=New Window\n\
        Exec=\"example studio\" --new\n";

    #[test]
    fn exec_arguments_are_quoted_as_the_specification_asks() {
        assert_eq!(exec_quote("%u"), "%u");
        assert_eq!(exec_quote("--open"), "--open");
        assert_eq!(exec_quote("100%"), "100%%");
        assert_eq!(exec_quote("two words"), "\"two words\"");
        assert_eq!(exec_quote("a$b\"c"), "\"a\\$b\\\"c\"");
        // Then escaped as a string: the backslashes double.
        assert_eq!(escape(&exec_quote("a\\b")), "\"a\\\\\\\\b\"");
        assert_eq!(escape(" lead\tand\nlines"), "\\slead\\tand\\nlines");
    }

    #[test]
    fn every_exec_starts_the_appimage() {
        let image = Path::new("/home/sam/Apps/Example Studio-1.0.0-x86_64.AppImage");
        let entry = rewrite_entry(ENTRY, image);
        let quoted = "\"/home/sam/Apps/Example Studio-1.0.0-x86_64.AppImage\"";
        assert!(entry.contains(&format!("\nExec={quoted} --open %u\nTryExec=/home/sam/Apps/Example Studio-1.0.0-x86_64.AppImage\n")), "{entry}");
        assert!(
            entry.contains(&format!(
                "[Desktop Action new]\nName=New Window\nExec={quoted} --new\n"
            )),
            "{entry}"
        );
        assert_eq!(entry.matches("TryExec=").count(), 1);
        assert!(entry.contains("MimeType=x-scheme-handler/examplestudio;"));
        // Rewriting again changes nothing.
        assert_eq!(rewrite_entry(&entry, image), entry);
    }

    fn appdir(root: &Path) -> PathBuf {
        let appdir = root.join("mount");
        let files: &[(&str, &[u8])] = &[
            ("com.example.studio.desktop", ENTRY.as_bytes()),
            (
                "usr/share/mime/packages/com.example.studio.xml",
                b"<mime-info/>",
            ),
            (
                "usr/share/icons/hicolor/256x256/apps/com.example.studio.png",
                b"png",
            ),
            (
                "usr/share/icons/hicolor/scalable/apps/com.example.studio.svg",
                b"svg",
            ),
        ];
        for (path, bytes) in files {
            let path = appdir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        appdir
    }

    #[test]
    fn integrating_writes_once_and_removing_takes_it_all_away() {
        let dir = tempfile::tempdir().unwrap();
        let appdir = appdir(dir.path());
        let data = dir.path().join("share");
        let integration = Integration {
            id: id_of(&appdir).unwrap(),
            appimage: dir.path().join("Example.AppImage"),
            appdir: appdir.clone(),
            data_home: data.clone(),
            refresh: false,
        };
        assert_eq!(integration.id, "com.example.studio");

        assert!(integration.install().unwrap());
        let entry =
            std::fs::read_to_string(data.join("applications/com.example.studio.desktop")).unwrap();
        assert!(entry.contains(&format!(
            "Exec={} --open %u",
            dir.path().join("Example.AppImage").display()
        )));
        assert_eq!(
            std::fs::read(data.join("mime/packages/com.example.studio.xml")).unwrap(),
            b"<mime-info/>"
        );
        assert!(
            data.join("icons/hicolor/256x256/apps/com.example.studio.png")
                .is_file()
        );
        assert!(
            data.join("icons/hicolor/scalable/apps/com.example.studio.svg")
                .is_file()
        );

        // Again: nothing to do.
        assert!(!integration.install().unwrap());

        // A release without MIME types or an SVG icon takes them away; one
        // moved elsewhere points at its new place.
        std::fs::remove_file(appdir.join("usr/share/mime/packages/com.example.studio.xml"))
            .unwrap();
        std::fs::remove_file(
            appdir.join("usr/share/icons/hicolor/scalable/apps/com.example.studio.svg"),
        )
        .unwrap();
        let moved = Integration {
            appimage: dir.path().join("Moved.AppImage"),
            ..integration.clone()
        };
        assert!(moved.install().unwrap());
        assert!(!data.join("mime/packages/com.example.studio.xml").exists());
        assert!(
            !data
                .join("icons/hicolor/scalable/apps/com.example.studio.svg")
                .exists()
        );
        let entry =
            std::fs::read_to_string(data.join("applications/com.example.studio.desktop")).unwrap();
        assert!(entry.contains("Moved.AppImage"));

        assert!(moved.remove().unwrap());
        assert!(
            !data
                .join("applications/com.example.studio.desktop")
                .exists()
        );
        assert!(
            !data
                .join("icons/hicolor/256x256/apps/com.example.studio.png")
                .exists()
        );
        assert!(!moved.remove().unwrap());
    }

    #[test]
    fn a_renamed_release_repoints_the_entry_and_keeps_everything_else() {
        let dir = tempfile::tempdir().unwrap();
        let appdir = appdir(dir.path());
        let data = dir.path().join("share");
        let old = dir.path().join("Example-1.0.0-x86_64.AppImage");
        let new = dir.path().join("Example-2.0.0-x86_64.AppImage");
        let integration = Integration {
            id: id_of(&appdir).unwrap(),
            appimage: old.clone(),
            appdir: appdir.clone(),
            data_home: data.clone(),
            refresh: false,
        };
        assert!(integration.install().unwrap());
        let other = data.join("applications/org.example.other.desktop");
        std::fs::write(&other, "[Desktop Entry]\nExec=other\n").unwrap();
        let files = |data: &Path| {
            let mut found = Vec::new();
            let mut folders = vec![data.to_path_buf()];
            while let Some(folder) = folders.pop() {
                for entry in std::fs::read_dir(folder).unwrap().flatten() {
                    if entry.path().is_dir() {
                        folders.push(entry.path());
                    } else {
                        found.push(entry.path());
                    }
                }
            }
            found.sort();
            found
        };
        let before = files(&data);

        let renamed = Integration {
            appimage: new.clone(),
            ..integration
        };
        assert!(renamed.install().unwrap());
        assert_eq!(files(&data), before, "a file was added or removed");
        let entry =
            std::fs::read_to_string(data.join("applications/com.example.studio.desktop")).unwrap();
        assert!(entry.contains(&format!("Exec={} --open %u", new.display())));
        assert!(entry.contains(&format!("TryExec={}", new.display())));
        assert!(!entry.contains("1.0.0"));
        assert!(!renamed.install().unwrap());
    }

    #[test]
    fn an_identifier_names_files_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let integration = Integration {
            id: "../../evil".into(),
            appimage: dir.path().join("x"),
            appdir: dir.path().to_path_buf(),
            data_home: dir.path().to_path_buf(),
            refresh: false,
        };
        assert!(matches!(integration.install(), Err(Error::Config(_))));
        assert!(matches!(integration.remove(), Err(Error::Config(_))));
        // An AppDir with no entry, or two, names no application.
        assert!(matches!(id_of(dir.path()), Err(Error::Config(_))));
    }
}
