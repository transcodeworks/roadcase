//! The macOS `.app`: its layout, Info.plist, programs, resources, extensions
//! and launch agents, and the plan for signing it inside out; then the
//! release made from it: dSYMs, signing, notarization, the disk image and
//! the update archive ([`release`]).
//!
//! ```text
//! <name>.app/Contents/
//!   Info.plist, PkgInfo
//!   MacOS/<executable>, MacOS/<program>...
//!   Resources/<name>.icns, Resources/Assets.car, resources...
//!   PlugIns/<extension>.appex/Contents/{Info.plist, MacOS/<executable>}
//!   Library/LaunchAgents/<label>.plist
//! ```

use crate::config::{Extension, LaunchAgent};
use crate::error::io;
use crate::{Error, Project, Result, files, info};
use plist::{Dictionary, Value};
use std::path::{Path, PathBuf};

mod release;
mod signing;
pub use release::*;
pub use signing::*;

/// An assembled application bundle, unsigned until [`sign`] runs.
#[derive(Debug, Clone)]
pub struct AppBundle {
    /// The `.app`.
    pub path: PathBuf,
    /// What to sign, in order: nested code first, the bundle last.
    pub signing: Vec<SignStep>,
    /// Executables whose symbols are kept as a dSYM, then stripped.
    pub symbols: Vec<PathBuf>,
}

/// One piece of code to sign.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignStep {
    /// The file or bundle.
    pub path: PathBuf,
    /// The entitlements as written in the config, placeholders not yet filled
    /// ([`render_entitlements`] fills them once the team is known).
    pub entitlements: Option<PathBuf>,
    /// A signing identifier other than the default.
    pub identifier: Option<String>,
}

/// Assembles `<out>/<name>.app`, replacing one already there. Nothing is
/// built, stripped or signed.
///
/// # Errors
///
/// [`Error::Missing`] when a file the config names is not there;
/// [`Error::Plist`]; [`Error::Value`]; [`Error::Io`].
pub fn assemble(project: &Project, out: &Path) -> Result<AppBundle> {
    let macos = project.macos()?;
    let context = &project.context;
    let app = out.join(format!("{}.app", context.name()));
    if app.exists() {
        std::fs::remove_dir_all(&app).map_err(io(&app))?;
    }
    let contents = app.join("Contents");
    let binaries = contents.join("MacOS");
    let resources = contents.join("Resources");
    for folder in [&binaries, &resources] {
        std::fs::create_dir_all(folder).map_err(io(folder))?;
    }
    let entitlements =
        |path: &Option<PathBuf>| path.as_ref().map(|path| context.path(path)).transpose();

    let mut signing = Vec::new();
    let mut symbols = Vec::new();

    // Programs come before everything that contains them.
    for program in &macos.programs {
        let from = files::resolve(context, &program.from, &format!("program {}", program.name))?;
        let to = binaries.join(&program.name);
        files::copy_file(&from, &to, true)?;
        if program.symbols {
            symbols.push(to.clone());
        }
        if program.sign {
            signing.push(SignStep {
                path: to,
                entitlements: entitlements(&program.entitlements)?,
                identifier: program
                    .identifier
                    .as_deref()
                    .map(|identifier| context.fill(identifier))
                    .transpose()?,
            });
        }
    }

    let executable = info::executable_name(project)?;
    let from = files::resolve(context, &macos.executable.from, "macos.executable")?;
    let main = binaries.join(&executable);
    files::copy_file(&from, &main, true)?;
    if macos.executable.symbols {
        symbols.push(main);
    }

    for resource in &macos.resources {
        let to = contents.join(context.fill(&resource.to)?);
        files::copy_resource(context, &resource.from, &to)?;
    }
    if let Some(icon) = &macos.icon {
        let from = existing(context.path(icon)?, "macos.icon")?;
        files::copy_file(
            &from,
            &resources.join(format!("{}.icns", context.name())),
            false,
        )?;
    }
    if let Some(assets) = &macos.assets_car {
        let from = existing(context.path(assets)?, "macos.assets_car")?;
        files::copy_file(&from, &resources.join("Assets.car"), false)?;
    }

    for extension in &macos.extensions {
        let path = place_extension(project, extension, &contents.join("PlugIns"))?;
        signing.push(SignStep {
            path,
            entitlements: entitlements(&extension.entitlements)?,
            identifier: None,
        });
    }
    for agent in &macos.launch_agents {
        place_agent(project, agent, &contents.join("Library/LaunchAgents"))?;
    }

    files::write_dictionary(&contents.join("Info.plist"), info::app(project)?)?;
    let pkginfo = contents.join("PkgInfo");
    std::fs::write(&pkginfo, b"APPL????").map_err(io(&pkginfo))?;

    signing.push(SignStep {
        path: app.clone(),
        entitlements: entitlements(&macos.entitlements)?,
        identifier: None,
    });
    Ok(AppBundle {
        path: app,
        signing,
        symbols,
    })
}

fn existing(path: PathBuf, what: &str) -> Result<PathBuf> {
    if path.exists() {
        Ok(path)
    } else {
        Err(Error::Missing(format!(
            "{what}: {} is not there",
            path.display()
        )))
    }
}

fn place_extension(project: &Project, extension: &Extension, plugins: &Path) -> Result<PathBuf> {
    let context = &project.context;
    let what = format!("macos.extension {}", extension.name);
    let target = plugins.join(format!("{}.appex", extension.name));
    if let Some(bundle) = &extension.bundle {
        let from = context.path(bundle)?;
        if !from.is_dir() {
            return Err(Error::Missing(format!(
                "{what}: {} is not a bundle folder",
                from.display()
            )));
        }
        files::copy_dir(&from, &target)?;
        return Ok(target);
    }
    let (Some(source), Some(plist)) = (&extension.executable, &extension.info_plist) else {
        return Err(Error::Config(format!(
            "{what} needs either bundle, or executable and info_plist"
        )));
    };
    let from = files::resolve(context, source, &what)?;
    let executable = from
        .file_name()
        .ok_or_else(|| Error::Missing(format!("{what}: no file name")))?
        .to_string_lossy()
        .into_owned();
    files::copy_file(
        &from,
        &target.join("Contents/MacOS").join(&executable),
        true,
    )?;

    let mut info = files::read_dictionary(&context.path(plist)?)?;
    let mut filled = Value::Dictionary(std::mem::take(&mut info));
    context.fill_plist(&mut filled)?;
    let mut info = filled.into_dictionary().unwrap_or_default();
    if !matches!(info.get("NSExtension"), Some(Value::Dictionary(_))) {
        return Err(Error::Plist(format!(
            "{what}: its Info.plist has no NSExtension dictionary"
        )));
    }
    let id = match &extension.id {
        Some(id) => context.fill(id)?,
        None => format!("{}.{}", context.id(), extension.name),
    };
    let text = |value: &str| Value::String(value.to_owned());
    info.insert("CFBundleIdentifier".into(), text(&id));
    info.insert("CFBundleExecutable".into(), text(&executable));
    info.insert("CFBundlePackageType".into(), text("XPC!"));
    info.insert("CFBundleInfoDictionaryVersion".into(), text("6.0"));
    info.insert(
        "CFBundleShortVersionString".into(),
        text(&info::short_version(context.version())),
    );
    info.insert("CFBundleVersion".into(), text(context.build_number()));
    if !info.contains_key("CFBundleName") {
        info.insert("CFBundleName".into(), text(&extension.name));
    }
    for (key, value) in context.plist_table(&extension.plist)? {
        info.insert(key, value);
    }
    files::write_dictionary(&target.join("Contents/Info.plist"), info)?;
    Ok(target)
}

fn place_agent(project: &Project, agent: &LaunchAgent, folder: &Path) -> Result<PathBuf> {
    let context = &project.context;
    let label = context.fill(&agent.label)?;
    let mut plist = match &agent.from {
        Some(from) => {
            let mut value = Value::Dictionary(files::read_dictionary(&context.path(from)?)?);
            context.fill_plist(&mut value)?;
            value.into_dictionary().unwrap_or_default()
        }
        None => {
            let mut plist = Dictionary::new();
            if let Some(program) = &agent.program {
                plist.insert(
                    "BundleProgram".into(),
                    Value::String(context.fill(program)?),
                );
            }
            if !agent.arguments.is_empty() {
                let arguments = agent
                    .arguments
                    .iter()
                    .map(|argument| context.fill(argument).map(Value::String))
                    .collect::<Result<_>>()?;
                plist.insert("ProgramArguments".into(), Value::Array(arguments));
            }
            if agent.run_at_load {
                plist.insert("RunAtLoad".into(), Value::Boolean(true));
            }
            if agent.keep_alive {
                plist.insert("KeepAlive".into(), Value::Boolean(true));
            }
            plist.insert(
                "AssociatedBundleIdentifiers".into(),
                Value::Array(vec![Value::String(context.id().to_owned())]),
            );
            plist
        }
    };
    plist.insert("Label".into(), Value::String(label.clone()));
    for (key, value) in context.plist_table(&agent.plist)? {
        plist.insert(key, value);
    }
    let path = folder.join(format!("{label}.plist"));
    files::write_dictionary(&path, plist)?;
    Ok(path)
}

/// Writes each step's entitlements, placeholders filled, into `folder`, and
/// answers with the steps pointing at the written files. Signing runs this
/// once it knows the team.
///
/// # Errors
///
/// [`Error::Value`] when a placeholder (often `{team}`) cannot be filled;
/// [`Error::Plist`].
pub fn render_entitlements(
    project: &Project,
    steps: &[SignStep],
    folder: &Path,
) -> Result<Vec<SignStep>> {
    std::fs::create_dir_all(folder).map_err(io(folder))?;
    let mut rendered = Vec::with_capacity(steps.len());
    for (index, step) in steps.iter().enumerate() {
        let mut step = step.clone();
        if let Some(source) = &step.entitlements {
            let mut value = Value::Dictionary(files::read_dictionary(source)?);
            project.context.fill_plist(&mut value)?;
            let name = step
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let path = folder.join(format!("{index:02}-{name}.entitlements"));
            files::write_dictionary(&path, value.into_dictionary().unwrap_or_default())?;
            step.entitlements = Some(path);
        }
        rendered.push(step);
    }
    Ok(rendered)
}
