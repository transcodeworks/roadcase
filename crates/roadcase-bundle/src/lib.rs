//! # roadcase-bundle
//!
//! Packages a desktop application from one `roadcase.toml`: the macOS `.app`
//! with its programs, extensions and launch agents, each signed with its own
//! entitlements; the disk image; notarization; the update archive; the
//! Windows NSIS installer; and the Linux AppDir, AppImage, `.deb` and pacman
//! package, with their desktop entry, icons and MIME types. The schema is in
//! [`config`].
//!
//! ```no_run
//! # fn main() -> Result<(), roadcase_bundle::Error> {
//! use roadcase_bundle::{Options, Project, macos};
//!
//! let project = Project::load("roadcase.toml".as_ref(), Options::macos())?;
//! let app = macos::assemble(&project, "target/package".as_ref())?;
//! println!("{}", app.path.display());
//! # Ok(())
//! # }
//! ```
//!
//! A release for one system is made on that system: [`macos::release`]
//! (dSYMs, `codesign` inside out, notarization, the disk image, the update
//! archive), [`windows::installer`] (staging, the signing command, `makensis`
//! with the application's own script) and the [`linux`] functions (AppDir,
//! `.debug` files, AppImage, `.deb`, pacman package). [`publish`] then writes
//! the update feed from the same config. Every external command is built as a
//! [`process::Invocation`] first, so the command lines are tested on any
//! system.

pub mod commands;
pub mod config;
mod context;
mod error;
mod files;
pub mod info;
pub mod linux;
pub mod macos;
pub mod process;
pub mod publish;
mod validate;
pub mod windows;

pub use config::Config;
pub use context::{BUILTINS, Context, Environment, Host};
pub use error::{Error, Result};
pub use validate::problems;

use std::path::Path;

/// How a project is loaded.
#[derive(Debug, Clone)]
pub struct Options {
    /// The variant, from `[variant.<name>]`.
    pub variant: Option<String>,
    /// The machine packaged for.
    pub host: Host,
    /// Where environment variables come from.
    pub environment: Environment,
}

impl Options {
    /// Packaging for macOS on this machine, with the process environment.
    pub fn macos() -> Self {
        Self {
            variant: None,
            host: Host::macos(),
            environment: Environment::Process,
        }
    }

    /// Packaging for Windows, with the process environment.
    pub fn windows() -> Self {
        Self {
            variant: None,
            host: Host::windows(),
            environment: Environment::Process,
        }
    }

    /// Packaging for Linux on this machine, with the process environment.
    pub fn linux() -> Self {
        Self {
            variant: None,
            host: Host::linux(),
            environment: Environment::Process,
        }
    }

    /// The same, with a variant.
    pub fn with_variant(mut self, variant: Option<&str>) -> Self {
        self.variant = variant.map(str::to_owned);
        self
    }
}

/// A config that has been read, checked and resolved.
#[derive(Debug, Clone)]
pub struct Project {
    /// The config as written.
    pub config: Config,
    /// Its resolved values.
    pub context: Context,
    variant: Option<String>,
}

impl Project {
    /// Reads `roadcase.toml` at `path`.
    ///
    /// # Errors
    ///
    /// [`Error::Io`]; [`Error::Config`] when it does not read;
    /// [`Error::Invalid`] with every problem; [`Error::Value`].
    pub fn load(path: &Path, options: Options) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(error::io(path))?;
        let base = path.parent().unwrap_or(Path::new("."));
        let base = if base.as_os_str().is_empty() {
            Path::new(".")
        } else {
            base
        };
        Self::parse(&text, base, options).map_err(|error| match error {
            Error::Config(message) => Error::Config(format!("{}: {message}", path.display())),
            other => other,
        })
    }

    /// Reads a config's text, with paths relative to `base`.
    ///
    /// # Errors
    ///
    /// As [`Project::load`].
    pub fn parse(text: &str, base: &Path, options: Options) -> Result<Self> {
        let config: Config =
            toml::from_str(text).map_err(|error| Error::Config(error.to_string()))?;
        let found = problems(&config);
        if !found.is_empty() {
            return Err(Error::Invalid(found));
        }
        let context = Context::new(
            &config,
            base,
            options.variant.as_deref(),
            &options.host,
            options.environment,
        )?;
        Ok(Self {
            config,
            context,
            variant: options.variant,
        })
    }

    /// The chosen variant's name.
    pub fn variant_name(&self) -> Option<&str> {
        self.variant.as_deref()
    }

    /// The chosen variant.
    pub fn variant(&self) -> Option<&config::Variant> {
        self.variant
            .as_deref()
            .and_then(|name| self.config.variants.get(name))
    }

    /// The `[macos]` table.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] when there is none.
    pub fn macos(&self) -> Result<&config::Macos> {
        self.config
            .macos
            .as_ref()
            .ok_or_else(|| Error::Config("the config has no [macos]".into()))
    }

    /// The `[windows]` table.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] when there is none.
    pub fn windows(&self) -> Result<&config::Windows> {
        self.config
            .windows
            .as_ref()
            .ok_or_else(|| Error::Config("the config has no [windows]".into()))
    }

    /// The `[linux]` table.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] when there is none.
    pub fn linux(&self) -> Result<&config::Linux> {
        self.config
            .linux
            .as_ref()
            .ok_or_else(|| Error::Config("the config has no [linux]".into()))
    }

    /// The URL schemes, as the variant has them.
    pub fn url_schemes(&self) -> &[String] {
        self.variant()
            .and_then(|variant| variant.url_schemes.as_deref())
            .unwrap_or(&self.config.app.url_schemes)
    }
}
