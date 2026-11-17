//! The Linux AppDir, its `.debug` files, and the AppImage, `.deb` and pacman
//! package made from it.
//!
//! ```text
//! <name>.AppDir/
//!   AppRun                              starts usr/bin/<executable>
//!   <id>.desktop, <id>.png|.svg, .DirIcon
//!   usr/bin/<executable>, usr/bin/<program>...
//!   usr/share/applications/<id>.desktop
//!   usr/share/icons/hicolor/<size>/apps/<id>.png, scalable/apps/<id>.svg
//!   usr/share/mime/packages/<id>.xml
//!   usr/share/metainfo/<id>.metainfo.xml
//! ```
//!
//! The AppDir is assembled in Rust, so it can be made and looked at on any
//! system. Splitting the symbols and making the AppImage run Linux's own
//! tools (`objcopy`, `strip`, `appimagetool`) and happen on Linux only.

use crate::config::{Rank, Role, TypeOwnership};
use crate::error::io;
use crate::{Error, Project, Result, files};
pub use roadcase_update::desktop::{escape, exec_quote};
use std::path::{Path, PathBuf};

mod packages;
pub use packages::*;

/// An assembled AppDir.
#[derive(Debug, Clone)]
pub struct AppDir {
    /// The `.AppDir` folder.
    pub path: PathBuf,
    /// Executables whose symbols go into a `.debug` file before they are
    /// stripped.
    pub symbols: Vec<PathBuf>,
}

/// The main executable's name in `usr/bin`.
///
/// # Errors
///
/// [`Error::Config`] without `[linux]`; [`Error::Value`].
pub fn executable_name(project: &Project) -> Result<String> {
    let linux = project.linux()?;
    match &linux.executable.name {
        Some(name) => project.context.fill(name),
        None => Ok(default_executable_name(project.context.name())),
    }
}

/// `Example Studio` is `example-studio`.
pub fn default_executable_name(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
        .to_lowercase()
}

/// A document type as Linux sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MimeType {
    /// The MIME type, such as `application/x-example-project`.
    pub name: String,
    /// What people read: the document type's name.
    pub comment: String,
    /// `*.<extension>` for each extension.
    pub globs: Vec<String>,
    /// Whether the AppImage declares it in its shared-mime-info file: the
    /// application owns it, and the variant does not import types.
    pub declared: bool,
    /// Whether the desktop entry lists it in `MimeType`, so the
    /// application is offered to open it.
    pub handled: bool,
}

/// `application/x-<first extension>`, the MIME type of a document type that
/// names none.
pub fn default_mime_type(extensions: &[String]) -> Option<String> {
    extensions
        .first()
        .map(|extension| format!("application/x-{}", extension.to_ascii_lowercase()))
}

/// The document types, as the variant has them.
pub fn mime_types(project: &Project) -> Vec<MimeType> {
    let variant = project.variant();
    let imported = variant.and_then(|variant| variant.types) == Some(TypeOwnership::Imported);
    project
        .config
        .document_types
        .iter()
        .filter_map(|document| {
            let name = document
                .mime_type
                .clone()
                .or_else(|| default_mime_type(&document.extensions))?;
            let rank = variant
                .and_then(|variant| variant.document_rank)
                .unwrap_or(document.rank);
            Some(MimeType {
                name,
                comment: document.name.clone(),
                globs: document
                    .extensions
                    .iter()
                    .map(|extension| format!("*.{extension}"))
                    .collect(),
                declared: rank == Rank::Owner && !imported && !document.extensions.is_empty(),
                handled: rank != Rank::None && document.role != Role::None,
            })
        })
        .collect()
}

/// What the desktop entry's `MimeType` lists: `x-scheme-handler/<scheme>`
/// for each URL scheme, then the document types the application opens.
pub fn handled_types(project: &Project) -> Vec<String> {
    let mut types: Vec<String> = project
        .url_schemes()
        .iter()
        .map(|scheme| format!("x-scheme-handler/{}", scheme.to_ascii_lowercase()))
        .collect();
    types.extend(
        mime_types(project)
            .into_iter()
            .filter(|mime| mime.handled)
            .map(|mime| mime.name),
    );
    types
}

/// Desktop entry keys roadcase writes, which `[linux.desktop]` may not set.
pub const GENERATED_KEYS: &[&str] = &[
    "Type",
    "Name",
    "Comment",
    "Exec",
    "TryExec",
    "Icon",
    "Terminal",
    "Categories",
    "MimeType",
    "X-AppImage-Version",
];

/// Field codes a desktop entry's `Exec` passes files or URLs with; it may
/// hold one at most.
pub const FILE_FIELD_CODES: &[&str] = &["%f", "%F", "%u", "%U"];

/// What follows the executable in `Exec`, as configured or by default.
pub fn exec_arguments(project: &Project) -> Result<Vec<String>> {
    let linux = project.linux()?;
    if let Some(arguments) = &linux.exec_arguments {
        return arguments
            .iter()
            .map(|argument| project.context.fill(argument))
            .collect();
    }
    let documents = mime_types(project).iter().any(|mime| mime.handled);
    Ok(if !project.url_schemes().is_empty() {
        vec!["%u".into()]
    } else if documents {
        vec!["%F".into()]
    } else {
        Vec::new()
    })
}

/// The desktop entry, `<id>.desktop`, as the AppImage carries it.
///
/// # Errors
///
/// [`Error::Config`] without `[linux]`; [`Error::Value`].
pub fn desktop_entry(project: &Project) -> Result<String> {
    entry(project, true)
}

/// The desktop entry a `.deb` or pacman package installs: the AppImage's,
/// without `X-AppImage-Version`. Its `Exec` names the executable, which
/// `/usr/bin` holds a link to.
///
/// # Errors
///
/// [`Error::Config`] without `[linux]`; [`Error::Value`].
pub fn package_desktop_entry(project: &Project) -> Result<String> {
    entry(project, false)
}

fn entry(project: &Project, appimage: bool) -> Result<String> {
    let linux = project.linux()?;
    let context = &project.context;
    let mut entry = String::from("[Desktop Entry]\n");
    let mut line = |key: &str, value: &str| {
        entry.push_str(key);
        entry.push('=');
        entry.push_str(value);
        entry.push('\n');
    };
    line("Type", "Application");
    line("Name", &escape(context.name()));
    if let Some(comment) = &linux.comment {
        line("Comment", &escape(&context.fill(comment)?));
    }
    let mut exec = vec![exec_quote(&executable_name(project)?)];
    exec.extend(
        exec_arguments(project)?
            .iter()
            .map(|argument| exec_quote(argument)),
    );
    line("Exec", &escape(&exec.join(" ")));
    line("Icon", &escape(context.id()));
    line("Terminal", "false");
    let categories: Vec<String> = linux
        .categories
        .iter()
        .map(|category| context.fill(category))
        .collect::<Result<_>>()?;
    line("Categories", &list(&categories));
    let handled = handled_types(project);
    if !handled.is_empty() {
        line("MimeType", &list(&handled));
    }
    if appimage {
        line("X-AppImage-Version", &escape(context.version()));
    }
    for (key, value) in &linux.desktop {
        let value = match value {
            toml::Value::String(text) => escape(&context.fill(text)?),
            toml::Value::Boolean(flag) => flag.to_string(),
            toml::Value::Integer(number) => number.to_string(),
            toml::Value::Float(number) => number.to_string(),
            toml::Value::Array(items) => {
                let items = items
                    .iter()
                    .map(|item| match item {
                        toml::Value::String(text) => context.fill(text),
                        other => Ok(other.to_string()),
                    })
                    .collect::<Result<Vec<_>>>()?;
                list(&items)
            }
            other => {
                return Err(Error::Config(format!(
                    "linux.desktop.{key}: {other} is not a string, a boolean or a list"
                )));
            }
        };
        line(key, &value);
    }
    Ok(entry)
}

/// The shared-mime-info file declaring the document types the application
/// owns, or `None` when it owns none.
pub fn mime_info(project: &Project) -> Option<String> {
    let declared: Vec<MimeType> = mime_types(project)
        .into_iter()
        .filter(|mime| mime.declared)
        .collect();
    if declared.is_empty() {
        return None;
    }
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <mime-info xmlns=\"http://www.freedesktop.org/standards/shared-mime-info\">\n",
    );
    for mime in declared {
        xml.push_str(&format!(
            "  <mime-type type=\"{}\">\n    <comment>{}</comment>\n",
            xml_escape(&mime.name),
            xml_escape(&mime.comment)
        ));
        for glob in &mime.globs {
            xml.push_str(&format!("    <glob pattern=\"{}\"/>\n", xml_escape(glob)));
        }
        xml.push_str("  </mime-type>\n");
    }
    xml.push_str("</mime-info>\n");
    Some(xml)
}

/// AppRun: starts the executable with the arguments it was given.
pub fn app_run(executable: &str) -> String {
    format!(
        "#!/bin/sh\n\
         HERE=\"${{APPDIR:-$(dirname \"$(readlink -f \"$0\")\")}}\"\n\
         exec \"$HERE/usr/bin/\"{} \"$@\"\n",
        shell_quote(executable)
    )
}

/// Where an icon goes in the hicolor theme: `scalable` for an SVG, `<n>x<n>`
/// for a square PNG.
///
/// # Errors
///
/// [`Error::Config`] for another kind of file or a PNG that is not square;
/// [`Error::Io`].
pub fn icon_slot(path: &Path) -> Result<(String, &'static str)> {
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase());
    match extension.as_deref() {
        Some("svg") => Ok(("scalable".into(), "svg")),
        Some("png") => {
            let bytes = std::fs::read(path).map_err(io(path))?;
            match png_size(&bytes) {
                Some((width, height)) if width == height => {
                    Ok((format!("{width}x{height}"), "png"))
                }
                Some((width, height)) => Err(Error::Config(format!(
                    "linux.icons: {} is {width}x{height}, and an icon is square",
                    path.display()
                ))),
                None => Err(Error::Config(format!(
                    "linux.icons: {} is not a PNG",
                    path.display()
                ))),
            }
        }
        _ => Err(Error::Config(format!(
            "linux.icons: {} is neither a .png nor an .svg",
            path.display()
        ))),
    }
}

/// A PNG's width and height, from its header.
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[..8] != SIGNATURE || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let number =
        |at: usize| u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    Some((number(16), number(20)))
}

/// Assembles `<out>/<name>.AppDir`, replacing one already there. Nothing is
/// built or stripped.
///
/// # Errors
///
/// [`Error::Missing`] when a file the config names is not there;
/// [`Error::Config`] for an icon that cannot be placed; [`Error::Value`];
/// [`Error::Io`].
pub fn assemble(project: &Project, out: &Path) -> Result<AppDir> {
    let linux = project.linux()?;
    let context = &project.context;
    let id = context.id();
    let root = out.join(format!("{}.AppDir", context.name()));
    if root.exists() {
        std::fs::remove_dir_all(&root).map_err(io(&root))?;
    }
    let bin = root.join("usr/bin");
    let share = root.join("usr/share");
    std::fs::create_dir_all(&bin).map_err(io(&bin))?;

    let mut symbols = Vec::new();
    for program in &linux.programs {
        let from = files::resolve(context, &program.from, &format!("program {}", program.name))?;
        let to = bin.join(&program.name);
        files::copy_file(&from, &to, true)?;
        if program.symbols {
            symbols.push(to);
        }
    }
    let executable = executable_name(project)?;
    let from = files::resolve(context, &linux.executable.from, "linux.executable")?;
    let main = bin.join(&executable);
    files::copy_file(&from, &main, true)?;
    if linux.executable.symbols {
        symbols.push(main);
    }

    for resource in &linux.resources {
        let to = root.join(context.fill(&resource.to)?);
        files::copy_resource(context, &resource.from, &to)?;
    }

    // The icons, and the one appimagetool shows: the largest PNG, else the SVG.
    let mut top: Option<(u32, PathBuf, &str)> = None;
    let mut slots = std::collections::BTreeSet::new();
    for icon in &linux.icons {
        let from = context.path(icon)?;
        if !from.is_file() {
            return Err(Error::Missing(format!(
                "linux.icons: {} is not there",
                from.display()
            )));
        }
        let (slot, extension) = icon_slot(&from)?;
        if !slots.insert(slot.clone()) {
            return Err(Error::Config(format!("linux.icons: two icons are {slot}")));
        }
        let to = share
            .join("icons/hicolor")
            .join(&slot)
            .join("apps")
            .join(format!("{id}.{extension}"));
        files::copy_file(&from, &to, false)?;
        let rank = slot
            .split('x')
            .next()
            .and_then(|size| size.parse().ok())
            .unwrap_or(0);
        if top.as_ref().is_none_or(|(best, _, _)| rank > *best) {
            top = Some((rank, to, extension));
        }
    }
    let Some((_, icon, extension)) = top else {
        return Err(Error::Missing(
            "linux.icons: an AppImage needs an icon".into(),
        ));
    };
    let top_icon = root.join(format!("{id}.{extension}"));
    files::copy_file(&icon, &top_icon, false)?;
    dir_icon(&root, &format!("{id}.{extension}"))?;

    let desktop = desktop_entry(project)?;
    write(&root.join(format!("{id}.desktop")), &desktop)?;
    write(
        &share.join("applications").join(format!("{id}.desktop")),
        &desktop,
    )?;
    if let Some(xml) = mime_info(project) {
        write(&share.join("mime/packages").join(format!("{id}.xml")), &xml)?;
    }
    if let Some(metainfo) = &linux.metainfo {
        let from = context.path(metainfo)?;
        let text = std::fs::read_to_string(&from).map_err(io(&from))?;
        write(
            &share.join("metainfo").join(format!("{id}.metainfo.xml")),
            &context.fill(&text)?,
        )?;
    }
    let run = root.join("AppRun");
    write(&run, &app_run(&executable))?;
    mark(&run)?;

    Ok(AppDir {
        path: root,
        symbols,
    })
}

#[cfg(unix)]
fn mark(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(io(path))
}

#[cfg(not(unix))]
fn mark(_: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn dir_icon(root: &Path, name: &str) -> Result<()> {
    let link = root.join(".DirIcon");
    std::os::unix::fs::symlink(name, &link).map_err(io(&link))
}

#[cfg(not(unix))]
fn dir_icon(root: &Path, name: &str) -> Result<()> {
    let link = root.join(".DirIcon");
    std::fs::copy(root.join(name), &link)
        .map(|_| ())
        .map_err(io(&link))
}

fn write(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    std::fs::write(path, text).map_err(io(path))
}

/// Moves each executable's symbols into `<out>/<name>.debug/<file>.debug`
/// (`objcopy --only-keep-debug`), strips it, links it to its `.debug` file
/// (`objcopy --add-gnu-debuglink`), and zips the `.debug` files as
/// `[linux.symbols] zip` says. `objcopy` and `strip` come from `PATH`.
///
/// # Errors
///
/// [`Error::Command`] when a tool is missing or fails; [`Error::Io`].
pub fn symbols(project: &Project, app: &AppDir, out: &Path) -> Result<PathBuf> {
    let linux = project.linux()?;
    let context = &project.context;
    let folder = out.join(format!("{}.debug", context.name()));
    if folder.exists() {
        std::fs::remove_dir_all(&folder).map_err(io(&folder))?;
    }
    std::fs::create_dir_all(&folder).map_err(io(&folder))?;
    let mut debug_files = Vec::new();
    for executable in &app.symbols {
        let name = executable
            .file_name()
            .ok_or_else(|| Error::Missing(format!("{} has no name", executable.display())))?;
        let debug = folder.join(format!("{}.debug", name.to_string_lossy()));
        tool(
            "objcopy",
            &[
                "--only-keep-debug".as_ref(),
                executable.as_os_str(),
                debug.as_os_str(),
            ],
        )?;
        tool("strip", &["--strip-all".as_ref(), executable.as_os_str()])?;
        let link = format!("--add-gnu-debuglink={}", debug.display());
        tool("objcopy", &[link.as_ref(), executable.as_os_str()])?;
        debug_files.push(debug);
    }
    let name = linux.symbols.clone().unwrap_or_default().zip;
    let zip = out.join(context.fill(&name)?);
    files::zip(&debug_files, &zip)?;
    Ok(zip)
}

fn tool(program: &str, arguments: &[&std::ffi::OsStr]) -> Result<()> {
    let output = std::process::Command::new(program)
        .args(arguments)
        .output()
        .map_err(|error| Error::Command(format!("{program}: {error}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Error::Command(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// Makes the AppImage from the AppDir with `appimagetool`, into `out`.
///
/// # Errors
///
/// [`Error::Config`] without `[linux.appimage]`; [`Error::Missing`] when no
/// runtime file is configured or it is not there; [`Error::Command`].
pub fn appimage(project: &Project, app: &AppDir, out: &Path) -> Result<PathBuf> {
    let linux = project.linux()?;
    let context = &project.context;
    let appimage = linux
        .appimage
        .as_ref()
        .ok_or_else(|| Error::Config("the config has no [linux.appimage]".into()))?;
    let runtime = context
        .resolve(&appimage.runtime, "linux.appimage.runtime")
        .map_err(|error| {
            Error::Missing(format!(
                "{error}: set $APPIMAGE_RUNTIME or linux.appimage.runtime to the AppImage runtime \
                 for this architecture, since appimagetool would otherwise download one"
            ))
        })?;
    let runtime = context.base.join(runtime);
    if !runtime.is_file() {
        return Err(Error::Missing(format!(
            "linux.appimage.runtime: {} is not there",
            runtime.display()
        )));
    }
    let tool = context.resolve(&appimage.tool, "linux.appimage.tool")?;
    std::fs::create_dir_all(out).map_err(io(out))?;
    let target = out.join(context.fill(&appimage.name)?);
    if target.exists() {
        std::fs::remove_file(&target).map_err(io(&target))?;
    }
    let output = std::process::Command::new(&tool)
        .arg("--runtime-file")
        .arg(&runtime)
        .arg(&app.path)
        .arg(&target)
        .env("ARCH", context.get("arch").unwrap_or_default())
        .env("VERSION", context.version())
        .output()
        .map_err(|error| Error::Command(format!("{tool}: {error}")))?;
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
    Ok(target)
}

/// A desktop entry list: each item escaped, `;` in items escaped, each item
/// followed by `;`.
pub fn list(items: &[String]) -> String {
    items
        .iter()
        .map(|item| format!("{};", escape(item).replace(';', "\\;")))
        .collect()
}

/// A word for `sh`, in single quotes.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_becomes_an_executable_name() {
        assert_eq!(default_executable_name("Example Studio"), "example-studio");
        assert_eq!(default_executable_name("Tiny"), "tiny");
    }

    #[test]
    fn strings_and_lists_are_escaped() {
        assert_eq!(
            list(&["AudioVideo".into(), "a;b".into()]),
            "AudioVideo;a\\;b;"
        );
    }

    #[test]
    fn app_run_starts_the_executable() {
        let script = app_run("it's");
        assert!(script.starts_with("#!/bin/sh\n"));
        assert!(script.contains("exec \"$HERE/usr/bin/\"'it'\\''s' \"$@\""));
    }

    #[test]
    fn a_png_header_gives_its_size() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&256u32.to_be_bytes());
        png.extend_from_slice(&128u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((256, 128)));
        assert_eq!(png_size(b"GIF89a"), None);
    }
}
