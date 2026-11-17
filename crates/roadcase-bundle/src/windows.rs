//! The Windows installer: what is installed, staged in a folder laid out as
//! the install folder; the signing command, run on each executable before
//! packaging and on the installer after; and `makensis` with the
//! application's own script, which is given defines and a generated include
//! of macros for the files, the registry and the update flags.
//!
//! ```text
//! <out>/<name>.windows/           the stage: <exe>, <program>..., <resource to>/...
//! <out>/<name>.windows.nsh        the generated include, ${ROADCASE_NSH}
//! <out>/<nsis.output>             the installer
//! ```
//!
//! Everything but running `makensis` and the signing command is plain Rust,
//! so the stage, the defines and the include are made, and tested, on any
//! system. Signing runs on Windows only.

use crate::config::{Rank, Signed, WindowsSign};
use crate::error::io;
use crate::process::Invocation;
use crate::{Context, Error, Project, Result, files};
use std::path::{Path, PathBuf};

/// The four numbers of a Windows `VIProductVersion`: `1.2.0-beta.1` is
/// `1.2.0.0`.
pub fn version4(version: &str) -> String {
    let core = version.split(['-', '+']).next().unwrap_or_default();
    let mut numbers: Vec<&str> = core.split('.').take(3).collect();
    while numbers.len() < 3 {
        numbers.push("0");
    }
    format!("{}.0", numbers.join("."))
}

/// The installed executable's file name.
///
/// # Errors
///
/// [`Error::Config`] without `[windows]`; [`Error::Value`].
pub fn executable_name(project: &Project) -> Result<String> {
    let windows = project.windows()?;
    match &windows.executable.name {
        Some(name) => project.context.fill(name),
        None => Ok(format!("{}.exe", project.context.name())),
    }
}

/// What an installer installs, staged.
#[derive(Debug, Clone)]
pub struct Stage {
    /// The folder, laid out as the install folder.
    pub folder: PathBuf,
    /// The main executable in it.
    pub executable: PathBuf,
    /// The programs beside it: file name, staged file, and whether the
    /// packager signs it.
    pub programs: Vec<(String, PathBuf, bool)>,
    /// The resource folders: `to` as the config has it, and the staged
    /// folder.
    pub resources: Vec<(String, PathBuf)>,
    /// Every staged file, relative to the folder, sorted.
    pub files: Vec<PathBuf>,
}

/// Stages `<out>/<name>.windows`, replacing one already there. Nothing is
/// built or signed.
///
/// # Errors
///
/// [`Error::Missing`] when a file the config names is not there;
/// [`Error::Value`]; [`Error::Io`].
pub fn stage(project: &Project, out: &Path) -> Result<Stage> {
    let windows = project.windows()?;
    let context = &project.context;
    std::fs::create_dir_all(out).map_err(io(out))?;
    let out = std::path::absolute(out).map_err(io(out))?;
    let folder = out.join(format!("{}.windows", context.name()));
    if folder.exists() {
        std::fs::remove_dir_all(&folder).map_err(io(&folder))?;
    }
    std::fs::create_dir_all(&folder).map_err(io(&folder))?;

    let from = files::resolve(context, &windows.executable.from, "windows.executable")?;
    let executable = folder.join(executable_name(project)?);
    files::copy_file(&from, &executable, false)?;

    let mut programs = Vec::new();
    for program in &windows.programs {
        let from = files::resolve(context, &program.from, &format!("program {}", program.name))?;
        let to = folder.join(&program.name);
        files::copy_file(&from, &to, false)?;
        programs.push((program.name.clone(), to, program.sign));
    }
    let mut resources = Vec::new();
    for resource in &windows.resources {
        let to = context.fill(&resource.to)?;
        let target = to
            .split(['/', '\\'])
            .filter(|part| !part.is_empty())
            .fold(folder.clone(), |path, part| path.join(part));
        files::copy_resource(context, &resource.from, &target)?;
        if !resources.iter().any(|(known, _)| *known == to) {
            resources.push((to, target));
        }
    }
    let mut files = Vec::new();
    walk(&folder, Path::new(""), &mut files)?;
    files.sort();
    Ok(Stage {
        folder,
        executable,
        programs,
        resources,
        files,
    })
}

fn walk(root: &Path, relative: &Path, found: &mut Vec<PathBuf>) -> Result<()> {
    let folder = root.join(relative);
    for entry in std::fs::read_dir(&folder).map_err(io(&folder))? {
        let entry = entry.map_err(io(&folder))?;
        let path = relative.join(entry.file_name());
        if entry.file_type().map_err(io(&folder))?.is_dir() {
            walk(root, &path, found)?;
        } else {
            found.push(path);
        }
    }
    Ok(())
}

/// A name as part of a define: upper case, everything but letters and digits
/// as `_`. `example-host.exe` is `EXAMPLE_HOST` (the `.exe` dropped).
pub fn define_name(name: &str) -> String {
    let name = name
        .strip_suffix(".exe")
        .or_else(|| name.strip_suffix(".EXE"))
        .unwrap_or(name);
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_uppercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_owned()
}

/// Stands for the installed executable until a command is written out.
const EXE: &str = "\u{1}exe\u{1}";

/// A registry command with `{exe}` and the config's placeholders filled;
/// [`EXE`] where the executable goes.
fn command_template(context: &Context, command: &str) -> Result<String> {
    let mut context = context.clone();
    context.set("exe", EXE.to_owned());
    context.fill(command)
}

/// The command a document opens with, and the one a URL opens with, as the
/// registry holds them: `{exe}` is `$INSTDIR\<executable>`, which NSIS
/// fills at install time.
///
/// # Errors
///
/// [`Error::Config`] without `[windows]`; [`Error::Value`].
pub fn open_commands(project: &Project) -> Result<(String, String)> {
    let registry = &project.windows()?.registry;
    let exe = format!("$INSTDIR\\{}", executable_name(project)?);
    let open = command_template(&project.context, &registry.open_command)?;
    let url = match &registry.url_command {
        Some(command) => command_template(&project.context, command)?,
        None => open.clone(),
    };
    Ok((open.replace(EXE, &exe), url.replace(EXE, &exe)))
}

/// The registry key of the uninstall entry, under `HKCU`.
///
/// # Errors
///
/// [`Error::Config`] without `[windows]`; [`Error::Value`].
pub fn uninstall_key(project: &Project) -> Result<String> {
    let name = match &project.windows()?.registry.uninstall_key {
        Some(key) => project.context.fill(key)?,
        None => project.context.name().to_owned(),
    };
    Ok(format!(
        "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{name}"
    ))
}

/// A document type as the registry holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Association {
    /// The extension, without its dot.
    pub extension: String,
    /// The ProgID: `prog_id`, or `<id>.<name without spaces>`.
    pub prog_id: String,
    /// What File Explorer calls the type.
    pub description: String,
    /// Whether the application becomes the default handler (`owner` and
    /// `default`), or is only added to Open With.
    pub default: bool,
}

/// The document types, one entry per extension, as the variant ranks them.
///
/// # Errors
///
/// [`Error::Value`].
pub fn associations(project: &Project) -> Result<Vec<Association>> {
    let context = &project.context;
    let rank_of = |document: &crate::config::DocumentType| {
        project
            .variant()
            .and_then(|variant| variant.document_rank)
            .unwrap_or(document.rank)
    };
    let mut found = Vec::new();
    for document in &project.config.document_types {
        let prog_id = match &document.prog_id {
            Some(prog_id) => context.fill(prog_id)?,
            None => format!(
                "{}.{}",
                context.id(),
                document
                    .name
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .collect::<String>()
            ),
        };
        for extension in &document.extensions {
            found.push(Association {
                extension: extension.clone(),
                prog_id: prog_id.clone(),
                description: document.name.clone(),
                default: matches!(rank_of(document), Rank::Owner | Rank::Default),
            });
        }
    }
    Ok(found)
}

/// The defines `makensis` is given, in order; see [`crate::config::Nsis`].
///
/// # Errors
///
/// [`Error::Missing`] when the icon is not there; [`Error::Value`].
pub fn defines(
    project: &Project,
    stage: &Stage,
    installer: &Path,
    include: &Path,
    signer: Option<&Signer>,
) -> Result<Vec<(String, String)>> {
    let windows = project.windows()?;
    let context = &project.context;
    let app = &project.config.app;
    let text = |path: &Path| path.to_string_lossy().into_owned();
    let mut defines: Vec<(String, String)> = Vec::new();
    let mut define = |name: &str, value: String| defines.push((name.to_owned(), value));

    define("NAME", context.name().to_owned());
    define("ID", context.id().to_owned());
    define("VERSION", context.version().to_owned());
    define("VERSION4", version4(context.version()));
    define(
        "PUBLISHER",
        match &app.publisher {
            Some(publisher) => context.fill(publisher)?,
            None => context.name().to_owned(),
        },
    );
    if let Some(copyright) = &app.copyright {
        define("COPYRIGHT", context.fill(copyright)?);
    }
    define("OUT", text(installer));
    define("STAGE", text(&stage.folder));
    define("EXE", text(&stage.executable));
    define("EXE_NAME", executable_name(project)?);
    for (name, path, _) in &stage.programs {
        define(&format!("PROGRAM_{}", define_name(name)), text(path));
    }
    for (to, path) in &stage.resources {
        let name = define_name(to);
        let name = if name.is_empty() { "ROOT" } else { &name };
        define(&format!("RESOURCE_{name}"), text(path));
    }
    if let Some(icon) = &windows.icon {
        let icon = context.path(icon)?;
        if !icon.is_file() {
            return Err(Error::Missing(format!(
                "windows.icon: {} is not there",
                icon.display()
            )));
        }
        define(
            "ICON",
            text(&std::path::absolute(&icon).map_err(io(&icon))?),
        );
    }
    define("INSTALL_MODE", "current-user".into());
    define("UNINSTALL_KEY", uninstall_key(project)?);
    let (open, url) = open_commands(project)?;
    define("OPEN_COMMAND", open);
    define("URL_COMMAND", url);
    let schemes = project.url_schemes();
    if !schemes.is_empty() {
        define("URL_SCHEMES", schemes.join(" "));
    }
    let flags = &windows.nsis.update_flags;
    define("SILENT_FLAG", flags.silent.clone());
    define("UPDATE_FLAG", flags.update.clone());
    define("RELAUNCH_FLAG", flags.relaunch.clone());
    for (index, flag) in flags.also_accept.iter().enumerate() {
        define(&format!("ALSO_ACCEPT_{}", index + 1), flag.clone());
    }
    if let Some(signer) = signer.filter(|signer| signer.signs(Signed::Uninstaller)) {
        define("SIGN_UNINSTALLER", signer.uninstaller_command());
    }
    define("ROADCASE_NSH", text(include));
    for (name, value) in &windows.nsis.defines {
        let value = context.resolve(value, &format!("windows.nsis.defines.{name}"))?;
        define(name, value);
    }
    Ok(defines)
}

/// A string for an NSIS script: in double quotes, `$` and `"` escaped.
pub fn nsis_string(text: &str) -> String {
    format!("\"{}\"", nsis_escape(text))
}

fn nsis_escape(text: &str) -> String {
    text.replace('$', "$$")
        .replace('"', "$\\\"")
        .replace('\r', "$\\r")
        .replace('\n', "$\\n")
}

/// A registry command for an NSIS script: escaped, with `$INSTDIR` left for
/// NSIS to fill.
fn nsis_command(context: &Context, command: &str, executable: &str) -> Result<String> {
    let template = command_template(context, command)?;
    Ok(format!(
        "\"{}\"",
        nsis_escape(&template).replace(EXE, &format!("$INSTDIR\\{executable}"))
    ))
}

/// A staged file's path under `$INSTDIR`, with backslashes.
fn installed(relative: &Path) -> String {
    relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("\\")
}

const INCLUDE_HEAD: &str = r#"!include "LogicLib.nsh"
!include "FileFunc.nsh"

; A file installed under NAME in $INSTDIR. One that is running cannot be
; replaced, but can be renamed: it is moved aside as NAME.<n>.old, which the
; next install or the uninstaller removes.
!macro ROADCASE_REPLACE NAME SOURCE
  Delete "$INSTDIR\${NAME}"
  ${If} ${FileExists} "$INSTDIR\${NAME}"
    StrCpy $R9 0
    ${Do}
      IntOp $R9 $R9 + 1
      Delete "$INSTDIR\${NAME}.$R9.old"
      ClearErrors
      Rename "$INSTDIR\${NAME}" "$INSTDIR\${NAME}.$R9.old"
      ${IfNot} ${Errors}
        ${Break}
      ${EndIf}
      ${If} $R9 >= 9
        ${Break}
      ${EndIf}
    ${Loop}
  ${EndIf}
  File "/oname=${NAME}" "${SOURCE}"
!macroend

; Waits until the installed executable can be written, for up to ten
; seconds: an application that started an update may still be leaving. Sets
; the error flag when it never can. Uses $R8 and $R9.
!macro ROADCASE_WAIT_UNTIL_WRITABLE
  StrCpy $R9 0
  ${Do}
    ClearErrors
    ${IfNot} ${FileExists} "$INSTDIR\${EXE_NAME}"
      ${Break}
    ${EndIf}
    FileOpen $R8 "$INSTDIR\${EXE_NAME}" a
    ${IfNot} ${Errors}
      FileClose $R8
      ${Break}
    ${EndIf}
    IntOp $R9 $R9 + 1
    ${If} $R9 > 50
      SetErrors
      ${Break}
    ${EndIf}
    Sleep 200
  ${Loop}
!macroend
"#;

/// The generated include, `${ROADCASE_NSH}`: macros for the staged files,
/// the registry entries of the URL schemes and document types, and the
/// update flags. A script uses those it wants.
///
/// # Errors
///
/// [`Error::Config`] without `[windows]`; [`Error::Value`].
pub fn include(project: &Project, stage: &Stage) -> Result<String> {
    let windows = project.windows()?;
    let context = &project.context;
    let executable = executable_name(project)?;
    let mut text = format!(
        "; Generated by roadcase for {} {}. Written again by every release: do not edit.\n",
        context.name(),
        context.version()
    );
    text.push_str(INCLUDE_HEAD);

    // The files: those at the top may be running programs; the rest are
    // written over.
    text.push_str(
        "\n; Every staged file, into $INSTDIR.\n!macro ROADCASE_INSTALL_FILES\n  SetOutPath \"$INSTDIR\"\n  Delete \"$INSTDIR\\*.old\"\n",
    );
    let mut folders: Vec<PathBuf> = Vec::new();
    let mut current = PathBuf::new();
    for file in &stage.files {
        let source = stage.folder.join(file).to_string_lossy().into_owned();
        let parent = file.parent().unwrap_or(Path::new("")).to_path_buf();
        if parent.as_os_str().is_empty() {
            if !current.as_os_str().is_empty() {
                text.push_str("  SetOutPath \"$INSTDIR\"\n");
                current = PathBuf::new();
            }
            text.push_str(&format!(
                "  !insertmacro ROADCASE_REPLACE {} {}\n",
                nsis_string(&installed(file)),
                nsis_string(&source)
            ));
            continue;
        }
        if parent != current {
            text.push_str(&format!(
                "  SetOutPath \"$INSTDIR\\{}\"\n",
                nsis_escape(&installed(&parent))
            ));
            current = parent.clone();
        }
        text.push_str(&format!("  File {}\n", nsis_string(&source)));
        for ancestor in parent.ancestors() {
            if !ancestor.as_os_str().is_empty() && !folders.iter().any(|known| known == ancestor) {
                folders.push(ancestor.to_path_buf());
            }
        }
    }
    text.push_str("  SetOutPath \"$INSTDIR\"\n!macroend\n");

    text.push_str(
        "\n; Removes them again, and the folders they were in once those are empty.\n!macro ROADCASE_UNINSTALL_FILES\n",
    );
    for file in &stage.files {
        text.push_str(&format!(
            "  Delete \"$INSTDIR\\{}\"\n",
            nsis_escape(&installed(file))
        ));
    }
    text.push_str("  Delete /REBOOTOK \"$INSTDIR\\*.old\"\n");
    // Deepest first.
    folders.sort_by_key(|folder| std::cmp::Reverse(folder.components().count()));
    for folder in &folders {
        text.push_str(&format!(
            "  RMDir \"$INSTDIR\\{}\"\n",
            nsis_escape(&installed(folder))
        ));
    }
    text.push_str("!macroend\n");

    // The registry: under HKCU, so that no administrator is needed.
    let registry = &windows.registry;
    let open = nsis_command(context, &registry.open_command, &executable)?;
    let url = match &registry.url_command {
        Some(command) => nsis_command(context, command, &executable)?,
        None => open.clone(),
    };
    let icon = format!("\"$\\\"$INSTDIR\\{}$\\\",0\"", nsis_escape(&executable));
    let classes = "Software\\Classes";
    let mut register = String::new();
    let mut unregister = String::new();
    for scheme in project.url_schemes() {
        let key = nsis_escape(&format!("{classes}\\{scheme}"));
        register.push_str(&format!(
            "  WriteRegStr HKCU \"{key}\" \"\" {}\n  WriteRegStr HKCU \"{key}\" \"URL Protocol\" \"\"\n  WriteRegStr HKCU \"{key}\\DefaultIcon\" \"\" {icon}\n  WriteRegStr HKCU \"{key}\\shell\\open\\command\" \"\" {url}\n",
            nsis_string(&format!("URL:{}", context.name())),
        ));
        // Only when the scheme is still this application's.
        unregister.push_str(&format!(
            "  ReadRegStr $R0 HKCU \"{key}\\shell\\open\\command\" \"\"\n  ${{If}} $R0 == {url}\n    DeleteRegKey HKCU \"{key}\"\n  ${{EndIf}}\n"
        ));
    }
    for association in associations(project)? {
        let extension = nsis_escape(&format!("{classes}\\.{}", association.extension));
        let prog_id = nsis_escape(&association.prog_id);
        let key = nsis_escape(&format!("{classes}\\{}", association.prog_id));
        if association.default {
            // The earlier default is kept in <ProgID>_backup, and put back
            // by the uninstaller.
            register.push_str(&format!(
                "  ReadRegStr $R0 HKCU \"{extension}\" \"\"\n  ${{If}} $R0 != \"{prog_id}\"\n    WriteRegStr HKCU \"{extension}\" \"{prog_id}_backup\" \"$R0\"\n  ${{EndIf}}\n  WriteRegStr HKCU \"{extension}\" \"\" \"{prog_id}\"\n"
            ));
        }
        register.push_str(&format!(
            "  WriteRegStr HKCU \"{extension}\\OpenWithProgids\" \"{prog_id}\" \"\"\n  WriteRegStr HKCU \"{key}\" \"\" {}\n  WriteRegStr HKCU \"{key}\\DefaultIcon\" \"\" {icon}\n  WriteRegStr HKCU \"{key}\\shell\" \"\" \"open\"\n  WriteRegStr HKCU \"{key}\\shell\\open\\command\" \"\" {open}\n",
            nsis_string(&association.description),
        ));
        unregister.push_str(&format!(
            "  ReadRegStr $R0 HKCU \"{extension}\" \"\"\n  ${{If}} $R0 == \"{prog_id}\"\n    ReadRegStr $R1 HKCU \"{extension}\" \"{prog_id}_backup\"\n    ${{If}} $R1 == \"\"\n      DeleteRegValue HKCU \"{extension}\" \"\"\n    ${{Else}}\n      WriteRegStr HKCU \"{extension}\" \"\" \"$R1\"\n    ${{EndIf}}\n  ${{EndIf}}\n  DeleteRegValue HKCU \"{extension}\" \"{prog_id}_backup\"\n  DeleteRegValue HKCU \"{extension}\\OpenWithProgids\" \"{prog_id}\"\n  DeleteRegKey /ifempty HKCU \"{extension}\\OpenWithProgids\"\n  DeleteRegKey /ifempty HKCU \"{extension}\"\n  DeleteRegKey HKCU \"{key}\"\n"
        ));
    }
    // SHCNE_ASSOCCHANGED: File Explorer reads the types again.
    let notify = "  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'\n";
    text.push_str(&format!(
        "\n; The URL schemes and document types, under HKCU\\Software\\Classes. Uses $R0.\n!macro ROADCASE_REGISTER\n{register}{notify}!macroend\n\n; Takes them away again, where they are still this application's. Uses $R0 and $R1.\n!macro ROADCASE_UNREGISTER\n{unregister}{notify}!macroend\n"
    ));

    // The update flags: /S is NSIS's own; the others are the updater's.
    let flags = &windows.nsis.update_flags;
    let mut read = String::from(
        "\n; How the installer was started. $R0 is 1 for an update (the application is\n; started again afterwards), $R1 the arguments to start it again with. Uses $R2\n; and $R3.\n!macro ROADCASE_READ_UPDATE\n  ${GetParameters} $R2\n  StrCpy $R0 0\n",
    );
    for flag in std::iter::once(&flags.update).chain(&flags.also_accept) {
        read.push_str(&format!(
            "  ClearErrors\n  ${{GetOptions}} $R2 {} $R3\n  ${{IfNot}} ${{Errors}}\n    StrCpy $R0 1\n  ${{EndIf}}\n",
            nsis_string(flag)
        ));
    }
    read.push_str(&format!(
        "  ClearErrors\n  ${{GetOptions}} $R2 {} $R1\n  ${{If}} ${{Errors}}\n    StrCpy $R1 \"\"\n  ${{EndIf}}\n  ClearErrors\n!macroend\n",
        nsis_string(&flags.relaunch)
    ));
    text.push_str(&read);
    Ok(text)
}

/// The signing command of `[windows.sign]`, ready to run on a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signer {
    /// The program and its arguments, `{file}` still in them.
    pub command: Vec<String>,
    /// Its environment.
    pub env: Vec<(String, String)>,
    /// What it signs.
    pub targets: Vec<Signed>,
}

impl Signer {
    /// Whether this kind of file is signed.
    pub fn signs(&self, what: Signed) -> bool {
        self.targets.contains(&what)
    }

    /// The command for one file.
    pub fn invocation(&self, file: &Path) -> Invocation {
        let file = file.to_string_lossy();
        let mut parts = self
            .command
            .iter()
            .map(|part| part.replace("{file}", &file));
        let mut invocation = Invocation::new(parts.next().unwrap_or_default()).args(parts);
        for (name, value) in &self.env {
            invocation = invocation.env(name.clone(), value.clone());
        }
        invocation
    }

    /// The command as one line for `!uninstfinalize`, which `makensis` runs
    /// with `%1` the uninstaller's path, in `makensis`'s environment.
    pub fn uninstaller_command(&self) -> String {
        self.command
            .iter()
            .map(|part| {
                let part = part.replace("{file}", "%1");
                if part.contains("%1") || part.contains(' ') || part.is_empty() {
                    format!("\"{part}\"")
                } else {
                    part
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The signer, when this run signs: on Windows (`native`), with
/// `[windows.sign]`, and with `when_env` set if the config names one. A
/// cross-built installer stays unsigned.
///
/// # Errors
///
/// [`Error::Value`] when the command or its environment cannot be resolved.
pub fn signer(project: &Project, native: bool) -> Result<Option<Signer>> {
    let Some(sign) = &project.windows()?.sign else {
        return Ok(None);
    };
    if !native || !wanted(project, sign) {
        return Ok(None);
    }
    let mut context = project.context.clone();
    context.set("file", "{file}".to_owned());
    let command = sign
        .command
        .iter()
        .map(|part| context.fill(part))
        .collect::<Result<Vec<_>>>()?;
    let env = sign
        .env
        .iter()
        .map(|(name, value)| {
            Ok((
                name.clone(),
                context.resolve(value, &format!("windows.sign.env.{name}"))?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Some(Signer {
        command,
        env,
        targets: sign.targets.clone(),
    }))
}

fn wanted(project: &Project, sign: &WindowsSign) -> bool {
    match &sign.when_env {
        Some(name) => project.context.environment().get(name).is_some(),
        None => true,
    }
}

/// `makensis`, with the defines and the script. `-D` rather than `/D`: both
/// read on Windows, and only `-D` elsewhere.
pub fn makensis_command(
    program: &str,
    defines: &[(String, String)],
    script: &Path,
    signer: Option<&Signer>,
) -> Invocation {
    let mut command = Invocation::new(program).arg("-V2");
    for (name, value) in defines {
        command = command.arg(format!("-D{name}={value}"));
    }
    command = command.path(script);
    // What !uninstfinalize runs needs the signing tool's environment.
    for (name, value) in signer
        .map(|signer| signer.env.as_slice())
        .unwrap_or_default()
    {
        command = command.env(name.clone(), value.clone());
    }
    command
}

/// What an installer build made.
#[derive(Debug, Clone)]
pub struct Installer {
    /// The installer.
    pub path: PathBuf,
    /// Whether the executables and the installer were signed.
    pub signed: bool,
}

/// Stages what is installed, signs each executable, writes the include,
/// runs `makensis` (from `$MAKENSIS`, or `PATH`) with the application's
/// script, and signs the installer, into `out`.
///
/// # Errors
///
/// [`Error::Missing`] when the script or a file is not there;
/// [`Error::Command`] when `makensis` or the signing command fails or writes
/// no installer; [`Error::Value`]; [`Error::Io`].
pub fn installer(project: &Project, out: &Path) -> Result<Installer> {
    let windows = project.windows()?;
    let context = &project.context;
    let script = context.path(&windows.nsis.script)?;
    if !script.is_file() {
        return Err(Error::Missing(format!(
            "windows.nsis.script: {} is not there",
            script.display()
        )));
    }
    let script = std::path::absolute(&script).map_err(io(&script))?;
    let stage = stage(project, out)?;
    let out = stage.folder.parent().unwrap_or(out).to_path_buf();
    let signer = signer(project, cfg!(windows))?;

    if let Some(signer) = &signer {
        if signer.signs(Signed::Executable) {
            signer.invocation(&stage.executable).run_all()?;
        }
        if signer.signs(Signed::Programs) {
            for (_, path, sign) in &stage.programs {
                if *sign {
                    signer.invocation(path).run_all()?;
                }
            }
        }
    }

    let include_path = out.join(format!("{}.windows.nsh", context.name()));
    std::fs::write(&include_path, include(project, &stage)?).map_err(io(&include_path))?;
    let installer = out.join(context.fill(&windows.nsis.output)?);
    if installer.exists() {
        std::fs::remove_file(&installer).map_err(io(&installer))?;
    }
    let defines = defines(project, &stage, &installer, &include_path, signer.as_ref())?;
    let program = context
        .environment()
        .get("MAKENSIS")
        .unwrap_or_else(|| "makensis".into());
    makensis_command(&program, &defines, &script, signer.as_ref())
        .run_all()
        .map_err(|error| match error {
            Error::Command(text) if !text.contains("failed (") => Error::Command(format!(
                "{text}: the installer is made with NSIS (https://nsis.sourceforge.io); put \
                 makensis on PATH, or name it in $MAKENSIS"
            )),
            other => other,
        })?;
    if !installer.is_file() {
        return Err(Error::Command(format!(
            "makensis succeeded and wrote no {}: the script's OutFile must be \"${{OUT}}\"",
            installer.display()
        )));
    }
    if let Some(signer) = signer.as_ref().filter(|s| s.signs(Signed::Installer)) {
        signer.invocation(&installer).run_all()?;
    }
    Ok(Installer {
        path: installer,
        signed: signer.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_has_four_numbers_for_windows() {
        assert_eq!(version4("1.2.3"), "1.2.3.0");
        assert_eq!(version4("1.2.0-beta.1"), "1.2.0.0");
        assert_eq!(version4("0.4.0+build.7"), "0.4.0.0");
    }

    #[test]
    fn names_become_defines() {
        assert_eq!(
            define_name("example-plugin-host.exe"),
            "EXAMPLE_PLUGIN_HOST"
        );
        assert_eq!(define_name("resources/docs"), "RESOURCES_DOCS");
        assert_eq!(define_name(""), "");
    }

    #[test]
    fn strings_are_escaped_for_nsis() {
        assert_eq!(nsis_string("plain"), "\"plain\"");
        assert_eq!(
            nsis_string("say \"hi\" for $5"),
            "\"say $\\\"hi$\\\" for $$5\""
        );
    }
}
