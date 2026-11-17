//! Placeholders and values, resolved once per run.

use crate::config::{Config, Lookup, Value};
use crate::{Error, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Placeholder names that `[vars]` may not take.
pub const BUILTINS: &[&str] = &[
    "name",
    "id",
    "version",
    "build_number",
    "arch",
    "apple_arch",
    "triple",
    "team",
    "channel",
];

/// What placeholders are filled with, and where relative paths start.
#[derive(Debug, Clone)]
pub struct Context {
    /// The folder that holds `roadcase.toml`.
    pub base: PathBuf,
    builtins: BTreeMap<String, String>,
    vars: BTreeMap<String, Value>,
    environment: Environment,
}

/// Where environment variables come from: the process, or a fixed map for tests.
#[derive(Debug, Clone, Default)]
pub enum Environment {
    /// The process environment.
    #[default]
    Process,
    /// Only these.
    Fixed(BTreeMap<String, String>),
}

impl Environment {
    /// A variable, when set and not empty.
    pub fn get(&self, name: &str) -> Option<String> {
        let value = match self {
            Self::Process => std::env::var(name).ok(),
            Self::Fixed(map) => map.get(name).cloned(),
        };
        value.filter(|value| !value.is_empty())
    }
}

/// The machine the package is for: this one, since every system packages
/// natively.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    /// `aarch64` or `x86_64`, as Rust names it.
    pub arch: String,
    /// The Rust target triple.
    pub triple: String,
}

impl Host {
    /// This machine, as its macOS triple: what packaging for macOS needs.
    pub fn macos() -> Self {
        let arch = std::env::consts::ARCH.to_owned();
        Self {
            triple: format!("{arch}-apple-darwin"),
            arch,
        }
    }

    /// This machine, as its Linux (glibc) triple: what packaging for Linux
    /// needs.
    pub fn linux() -> Self {
        let arch = std::env::consts::ARCH.to_owned();
        Self {
            triple: format!("{arch}-unknown-linux-gnu"),
            arch,
        }
    }

    /// x86_64 Windows, the one Windows target.
    pub fn windows() -> Self {
        Self {
            arch: "x86_64".into(),
            triple: "x86_64-pc-windows-msvc".into(),
        }
    }
}

impl Context {
    /// Resolves the application's own values from `config`, as `variant`
    /// changes them.
    ///
    /// # Errors
    ///
    /// [`Error::Value`] when a value cannot be resolved; [`Error::Config`]
    /// when the variant does not exist or the version is not semantic.
    pub fn new(
        config: &Config,
        base: &Path,
        variant: Option<&str>,
        host: &Host,
        environment: Environment,
    ) -> Result<Self> {
        let variant = match variant {
            Some(name) => Some(
                config
                    .variants
                    .get(name)
                    .ok_or_else(|| Error::Config(format!("there is no variant {name}")))?,
            ),
            None => None,
        };
        let mut vars = config.vars.clone();
        if let Some(variant) = variant {
            vars.extend(variant.vars.clone());
        }
        let mut context = Self {
            base: base.to_path_buf(),
            builtins: BTreeMap::new(),
            vars,
            environment,
        };
        let name = variant
            .and_then(|variant| variant.name.clone())
            .unwrap_or_else(|| config.app.name.clone());
        let id = variant
            .and_then(|variant| variant.id.clone())
            .unwrap_or_else(|| config.app.id.clone());
        context.set("name", name);
        context.set("id", id);
        context.set("arch", host.arch.clone());
        let apple_arch = if host.arch == "aarch64" {
            "arm64"
        } else {
            host.arch.as_str()
        };
        context.set("apple_arch", apple_arch.to_owned());
        context.set("triple", host.triple.clone());
        let version = context.resolve(&config.app.version, "app.version")?;
        semver::Version::parse(&version).map_err(|error| {
            Error::Config(format!(
                "app.version {version} is not a semantic version: {error}"
            ))
        })?;
        context.set("version", version.clone());
        let build_number = match &config.app.build_number {
            Some(value) => context.resolve(value, "app.build_number")?,
            None => version,
        };
        context.set("build_number", build_number);
        if let Some(feed) = &config.feed {
            let channel = context.resolve(&feed.channel, "feed.channel")?;
            context.set("channel", channel);
        }
        if let Some(team) = config
            .macos
            .as_ref()
            .and_then(|macos| macos.signing.team.as_ref())
        {
            let team = context.resolve(team, "macos.signing.team")?;
            // Empty: read from the certificate when signing.
            if !team.is_empty() {
                context.set("team", team);
            }
        }
        Ok(context)
    }

    /// Sets a built-in placeholder, such as `team` once signing knows it.
    pub fn set(&mut self, name: &str, value: String) {
        self.builtins.insert(name.to_owned(), value);
    }

    /// A built-in placeholder's value, if it has one.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.builtins.get(name).map(String::as_str)
    }

    /// The application's name, as the variant has it.
    pub fn name(&self) -> &str {
        self.get("name").unwrap_or_default()
    }

    /// The application's identifier, as the variant has it.
    pub fn id(&self) -> &str {
        self.get("id").unwrap_or_default()
    }

    /// The resolved version.
    pub fn version(&self) -> &str {
        self.get("version").unwrap_or_default()
    }

    /// The resolved build number.
    pub fn build_number(&self) -> &str {
        self.get("build_number").unwrap_or_default()
    }

    /// The environment this context reads.
    pub fn environment(&self) -> &Environment {
        &self.environment
    }

    /// A path from the config, placeholders filled, relative to its folder.
    ///
    /// # Errors
    ///
    /// [`Error::Value`] when a placeholder cannot be filled.
    pub fn path(&self, path: &Path) -> Result<PathBuf> {
        let text = self.fill(&path.to_string_lossy())?;
        Ok(self.base.join(text))
    }

    /// Resolves a [`Value`]. `what` names it in errors.
    ///
    /// # Errors
    ///
    /// [`Error::Value`] when nothing answers or a placeholder cannot be filled.
    pub fn resolve(&self, value: &Value, what: &str) -> Result<String> {
        self.resolve_with(value, what, &mut Vec::new())
    }

    fn resolve_with(&self, value: &Value, what: &str, seen: &mut Vec<String>) -> Result<String> {
        match value {
            Value::Text(text) => self.fill_with(text, seen),
            Value::Lookup(lookup) => self.look_up(lookup, what, seen),
        }
    }

    fn look_up(&self, lookup: &Lookup, what: &str, seen: &mut Vec<String>) -> Result<String> {
        if let Some(found) = lookup
            .env
            .as_deref()
            .and_then(|name| self.environment.get(name))
        {
            return Ok(found);
        }
        if let Some(file) = &lookup.file {
            let path = self
                .base
                .join(self.fill_with(&file.to_string_lossy(), seen)?);
            let text = std::fs::read_to_string(&path)
                .map_err(|error| Error::Value(format!("{what}: {}: {error}", path.display())))?;
            match &lookup.regex {
                None => return Ok(text.trim().to_owned()),
                Some(pattern) => {
                    let regex = regex::Regex::new(pattern)
                        .map_err(|error| Error::Value(format!("{what}: {error}")))?;
                    if let Some(found) = regex.captures(&text).and_then(|captures| captures.get(1))
                    {
                        return Ok(found.as_str().to_owned());
                    }
                }
            }
        }
        if let Some(default) = &lookup.default {
            return self.fill_with(default, seen);
        }
        let mut places = Vec::new();
        if let Some(env) = &lookup.env {
            places.push(format!("${env} is not set"));
        }
        if let Some(file) = &lookup.file {
            places.push(format!("{} does not hold it", file.display()));
        }
        Err(Error::Value(format!(
            "{what} has no value: {}",
            if places.is_empty() {
                "the lookup names no place".to_owned()
            } else {
                places.join(", ")
            }
        )))
    }

    /// Fills the placeholders in `text`.
    ///
    /// # Errors
    ///
    /// [`Error::Value`] for an unknown placeholder, an unset `{env:...}`,
    /// `{team}` before it is known, or a brace that is not closed.
    pub fn fill(&self, text: &str) -> Result<String> {
        self.fill_with(text, &mut Vec::new())
    }

    fn fill_with(&self, text: &str, seen: &mut Vec<String>) -> Result<String> {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(at) = rest.find(['{', '}']) {
            out.push_str(&rest[..at]);
            let tail = &rest[at..];
            if tail.starts_with("{{") || tail.starts_with("}}") {
                out.push_str(&tail[..1]);
                rest = &tail[2..];
                continue;
            }
            if tail.starts_with('}') {
                return Err(Error::Value(format!(
                    "{text}: a lone }} (write }}}} for one)"
                )));
            }
            let Some(end) = tail.find('}') else {
                return Err(Error::Value(format!("{text}: a {{ is not closed")));
            };
            let name = &tail[1..end];
            out.push_str(&self.placeholder(name, text, seen)?);
            rest = &tail[end + 1..];
        }
        out.push_str(rest);
        Ok(out)
    }

    fn placeholder(&self, name: &str, text: &str, seen: &mut Vec<String>) -> Result<String> {
        if let Some(variable) = name.strip_prefix("env:") {
            return self
                .environment
                .get(variable)
                .ok_or_else(|| Error::Value(format!("{text}: ${variable} is not set")));
        }
        if let Some(value) = self.builtins.get(name) {
            return Ok(value.clone());
        }
        if name == "team" {
            return Err(Error::Value(format!(
                "{text}: the team is not known yet: set macos.signing.team, or sign with an Apple identity"
            )));
        }
        if let Some(value) = self.vars.get(name) {
            if seen.iter().any(|found| found == name) {
                return Err(Error::Value(format!("vars.{name} refers to itself")));
            }
            seen.push(name.to_owned());
            let resolved = self.resolve_with(value, &format!("vars.{name}"), seen);
            seen.pop();
            return resolved;
        }
        Err(Error::Value(format!(
            "{text}: {{{name}}} is not a placeholder"
        )))
    }

    /// Converts a TOML value to a plist value, filling placeholders in strings.
    ///
    /// # Errors
    ///
    /// [`Error::Value`] from [`Context::fill`].
    pub fn plist_value(&self, value: &toml::Value) -> Result<plist::Value> {
        Ok(match value {
            toml::Value::String(text) => plist::Value::String(self.fill(text)?),
            toml::Value::Integer(number) => plist::Value::Integer((*number).into()),
            toml::Value::Float(number) => plist::Value::Real(*number),
            toml::Value::Boolean(flag) => plist::Value::Boolean(*flag),
            toml::Value::Datetime(time) => plist::Value::String(time.to_string()),
            toml::Value::Array(items) => plist::Value::Array(
                items
                    .iter()
                    .map(|item| self.plist_value(item))
                    .collect::<Result<_>>()?,
            ),
            toml::Value::Table(table) => plist::Value::Dictionary(self.plist_table(table)?),
        })
    }

    /// Converts a TOML table to a plist dictionary, as [`Context::plist_value`].
    ///
    /// # Errors
    ///
    /// [`Error::Value`] from [`Context::fill`].
    pub fn plist_table(&self, table: &toml::Table) -> Result<plist::Dictionary> {
        let mut dictionary = plist::Dictionary::new();
        for (key, value) in table {
            dictionary.insert(key.clone(), self.plist_value(value)?);
        }
        Ok(dictionary)
    }

    /// Fills placeholders in every string of a plist value, in place.
    ///
    /// # Errors
    ///
    /// [`Error::Value`] from [`Context::fill`].
    pub fn fill_plist(&self, value: &mut plist::Value) -> Result<()> {
        match value {
            plist::Value::String(text) => *text = self.fill(text)?,
            plist::Value::Array(items) => {
                for item in items {
                    self.fill_plist(item)?;
                }
            }
            plist::Value::Dictionary(dictionary) => {
                for (_, item) in dictionary.iter_mut() {
                    self.fill_plist(item)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(toml: &str, env: &[(&str, &str)]) -> Result<Context> {
        let config: Config = toml::from_str(toml).unwrap();
        let env = env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        Context::new(
            &config,
            Path::new("/base"),
            None,
            &Host {
                arch: "aarch64".into(),
                triple: "aarch64-apple-darwin".into(),
            },
            Environment::Fixed(env),
        )
    }

    const APP: &str = r#"
        [app]
        name = "Example"
        id = "com.example.app"
        version = { env = "APP_VERSION", default = "1.0.0" }
        [vars]
        group = "{team}.example"
        label = "{id}.agent"
        "#;

    #[test]
    fn placeholders_fill_from_builtins_vars_and_the_environment() {
        let context = context(APP, &[("APP_VERSION", "2.1.0"), ("HOME", "/h")]).unwrap();
        assert_eq!(context.version(), "2.1.0");
        assert_eq!(context.build_number(), "2.1.0");
        assert_eq!(
            context.fill("{name}-{version}-{arch}.dmg").unwrap(),
            "Example-2.1.0-aarch64.dmg"
        );
        assert_eq!(context.fill("{label}").unwrap(), "com.example.app.agent");
        assert_eq!(context.fill("{env:HOME}/x").unwrap(), "/h/x");
        assert_eq!(context.fill("{{literal}}").unwrap(), "{literal}");
    }

    #[test]
    fn what_cannot_be_filled_is_an_error() {
        let context = context(APP, &[]).unwrap();
        assert_eq!(context.version(), "1.0.0");
        for text in ["{group}", "{nope}", "{env:UNSET}", "{open", "close}"] {
            assert!(matches!(context.fill(text), Err(Error::Value(_))), "{text}");
        }
        let mut with_team = context.clone();
        with_team.set("team", "ABCDE12345".into());
        assert_eq!(with_team.fill("{group}").unwrap(), "ABCDE12345.example");
    }

    #[test]
    fn a_version_can_be_read_from_a_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nversion = \"3.4.5-beta.1\"\n",
        )
        .unwrap();
        let config: Config = toml::from_str(
            r#"
            [app]
            name = "Example"
            id = "com.example.app"
            version = { env = "APP_VERSION", file = "Cargo.toml", regex = '(?m)^version = "(.*)"' }
            "#,
        )
        .unwrap();
        let context = Context::new(
            &config,
            dir.path(),
            None,
            &Host::macos(),
            Environment::Fixed(BTreeMap::new()),
        )
        .unwrap();
        assert_eq!(context.version(), "3.4.5-beta.1");
    }

    #[test]
    fn a_version_must_be_semantic() {
        let toml = APP.replace("default = \"1.0.0\"", "default = \"1.0\"");
        assert!(matches!(context(&toml, &[]), Err(Error::Config(_))));
    }

    #[test]
    fn a_var_that_refers_to_itself_is_an_error() {
        let toml = format!("{APP}\nloop = \"{{loop}}\"\n");
        let context = context(&toml, &[]).unwrap();
        assert!(matches!(context.fill("{loop}"), Err(Error::Value(_))));
    }
}
