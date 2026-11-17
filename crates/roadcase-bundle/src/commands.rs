//! Running the commands a config declares.

use crate::config::Command;
use crate::{Error, Project, Result};

/// Runs `[app] build`, then each macOS extension's `build`, with the feed's
/// URL and public key in the environment names `[feed.build_env]` gives.
///
/// # Errors
///
/// [`Error::Command`] when a command does not start or fails; [`Error::Value`].
pub fn build(project: &Project, macos_extensions: bool) -> Result<()> {
    let mut commands: Vec<&Command> = project.config.app.build.iter().collect();
    if macos_extensions && let Some(macos) = &project.config.macos {
        commands.extend(
            macos
                .extensions
                .iter()
                .flat_map(|extension| &extension.build),
        );
    }
    let feed_env = feed_environment(project)?;
    for command in commands {
        run(project, command, &feed_env)?;
    }
    Ok(())
}

/// The feed's build-time environment: name and value pairs.
///
/// # Errors
///
/// [`Error::Value`]; [`Error::Config`] when the public key does not read.
pub fn feed_environment(project: &Project) -> Result<Vec<(String, String)>> {
    let Some(feed) = &project.config.feed else {
        return Ok(Vec::new());
    };
    let context = &project.context;
    let mut pairs = Vec::new();
    if let Some(name) = &feed.build_env.url {
        pairs.push((name.clone(), context.resolve(&feed.url, "feed.url")?));
    }
    if let Some(name) = &feed.build_env.key {
        let key = context.resolve(&feed.public_key, "feed.public_key")?;
        roadcase_update::PublicKey::parse(&key)
            .map_err(|error| Error::Config(format!("feed.public_key: {error}")))?;
        // The base64 line, also when the value is the whole file.
        let line = key.lines().last().unwrap_or_default().trim().to_owned();
        pairs.push((name.clone(), line));
    }
    Ok(pairs)
}

fn run(project: &Project, command: &Command, extra: &[(String, String)]) -> Result<()> {
    let context = &project.context;
    let parts = command
        .run
        .iter()
        .map(|part| context.fill(part))
        .collect::<Result<Vec<_>>>()?;
    let Some((program, arguments)) = parts.split_first() else {
        return Err(Error::Command("a build command is empty".into()));
    };
    let cwd = match &command.cwd {
        Some(cwd) => context.path(cwd)?,
        None => context.base.clone(),
    };
    let mut process = std::process::Command::new(program);
    process.args(arguments).current_dir(&cwd);
    for (name, value) in extra {
        process.env(name, value);
    }
    for (name, value) in &command.env {
        process.env(name, context.resolve(value, &format!("env {name}"))?);
    }
    let status = process
        .status()
        .map_err(|error| Error::Command(format!("{program}: {error}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Command(format!(
            "{} failed: {status}",
            parts.join(" ")
        )))
    }
}
