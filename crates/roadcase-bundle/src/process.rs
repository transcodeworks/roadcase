//! The commands packaging runs, as values: built first, so that tests can
//! read a command line on any system, then run.

use crate::{Error, Result};
use std::path::{Path, PathBuf};

/// One command: a program, its arguments, and what it runs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// The program, by path or by name on `PATH`.
    pub program: String,
    /// Its arguments, as they are passed: no shell, no splitting.
    pub arguments: Vec<String>,
    /// Extra environment.
    pub env: Vec<(String, String)>,
    /// Where it runs, when not the current folder.
    pub cwd: Option<PathBuf>,
    /// Argument values that never appear in an error, such as a password.
    pub secrets: Vec<String>,
}

impl Invocation {
    /// A command with no arguments yet.
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            arguments: Vec::new(),
            env: Vec::new(),
            cwd: None,
            secrets: Vec::new(),
        }
    }

    /// Adds an argument.
    #[must_use]
    pub fn arg(mut self, argument: impl Into<String>) -> Self {
        self.arguments.push(argument.into());
        self
    }

    /// Adds arguments.
    #[must_use]
    pub fn args<I, S>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.arguments.extend(arguments.into_iter().map(Into::into));
        self
    }

    /// Adds a path as an argument.
    #[must_use]
    pub fn path(self, path: &Path) -> Self {
        self.arg(path.to_string_lossy().into_owned())
    }

    /// Adds an argument that errors never show.
    #[must_use]
    pub fn secret(mut self, argument: impl Into<String>) -> Self {
        let argument = argument.into();
        self.secrets.push(argument.clone());
        self.arguments.push(argument);
        self
    }

    /// Sets an environment variable for it.
    #[must_use]
    pub fn env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((name.into(), value.into()));
        self
    }

    /// Sets the folder it runs in.
    #[must_use]
    pub fn cwd(mut self, folder: &Path) -> Self {
        self.cwd = Some(folder.to_path_buf());
        self
    }

    /// The command line for a person to read, with secrets left out.
    pub fn display(&self) -> String {
        let mut line = self.program.clone();
        for argument in &self.arguments {
            line.push(' ');
            if self.secrets.contains(argument) {
                line.push_str("***");
            } else if argument.is_empty() || argument.contains(' ') {
                line.push('"');
                line.push_str(argument);
                line.push('"');
            } else {
                line.push_str(argument);
            }
        }
        line
    }

    fn command(&self) -> std::process::Command {
        let mut command = std::process::Command::new(&self.program);
        command.args(&self.arguments);
        for (name, value) in &self.env {
            command.env(name, value);
        }
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        command
    }

    /// Runs it and answers with what it wrote to standard output.
    ///
    /// # Errors
    ///
    /// [`Error::Command`] when it does not start or fails, with what it wrote
    /// and no secret.
    pub fn run(&self) -> Result<String> {
        let output = self
            .command()
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|error| Error::Command(format!("{}: {error}", self.program)))?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        if output.status.success() {
            return Ok(stdout);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let mut said = [stdout.trim(), stderr.trim()]
            .into_iter()
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        for secret in self.secrets.iter().filter(|secret| !secret.is_empty()) {
            said = said.replace(secret.as_str(), "***");
        }
        Err(Error::Command(format!(
            "{} failed ({}){}{said}",
            self.display(),
            output.status,
            if said.is_empty() { "" } else { ":\n" }
        )))
    }

    /// Runs it and answers with standard output and standard error together,
    /// for tools that report on the second.
    ///
    /// # Errors
    ///
    /// As [`Invocation::run`].
    pub fn run_all(&self) -> Result<String> {
        let output = self
            .command()
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|error| Error::Command(format!("{}: {error}", self.program)))?;
        let mut said = String::from_utf8_lossy(&output.stdout).into_owned();
        said.push_str(&String::from_utf8_lossy(&output.stderr));
        if output.status.success() {
            return Ok(said);
        }
        for secret in self.secrets.iter().filter(|secret| !secret.is_empty()) {
            said = said.replace(secret.as_str(), "***");
        }
        Err(Error::Command(format!(
            "{} failed ({}):\n{}",
            self.display(),
            output.status,
            said.trim()
        )))
    }
}

/// Whether a program is on `PATH`.
pub fn on_path(program: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|folder| {
        let candidate = folder.join(program);
        candidate.is_file() || candidate.with_extension("exe").is_file()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_line_reads_without_its_secrets() {
        let invocation = Invocation::new("xcrun")
            .args(["notarytool", "submit"])
            .arg("My App.zip")
            .arg("--password")
            .secret("hunter2");
        assert_eq!(
            invocation.display(),
            "xcrun notarytool submit \"My App.zip\" --password ***"
        );
    }

    #[test]
    fn a_failure_says_what_ran_and_hides_secrets() {
        let failed = Invocation::new("definitely-not-a-program-roadcase")
            .secret("hunter2")
            .run()
            .unwrap_err();
        let text = failed.to_string();
        assert!(text.contains("definitely-not-a-program-roadcase"), "{text}");
        assert!(!text.contains("hunter2"), "{text}");
    }
}
