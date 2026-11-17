//! A tiny application for scripts/macos-e2e.sh and scripts/windows-e2e.ps1:
//! its version, feed and key are compiled in from FIXTURE_VERSION,
//! FIXTURE_FEED and FIXTURE_PUBLIC_KEY (roadcase passes the last two, as
//! `[feed.build_env]` says).
//!
//! - `version` prints the version;
//! - `helper` runs the helper program beside the executable and prints what
//!   it says;
//! - `update` checks the feed, downloads and installs a newer release, then
//!   leaves, asking to be started again with `relaunched`;
//! - `relaunched` appends its version to the log: `$FIXTURE_LOG`, or else
//!   `fixture.log` beside the `.app` (an application opened by macOS does not
//!   inherit this environment).

use roadcase_update::{Config, Updater};
use std::path::PathBuf;

const VERSION: &str = env!("FIXTURE_VERSION");
const FEED: &str = env!("FIXTURE_FEED");
const PUBLIC_KEY: &str = env!("FIXTURE_PUBLIC_KEY");

fn main() {
    let command = std::env::args().nth(1).unwrap_or_default();
    let result = match command.as_str() {
        "version" => {
            println!("{VERSION}");
            Ok(())
        }
        "helper" => helper(),
        "update" => update(),
        "relaunched" => relaunched(),
        other => Err(format!("unknown command {other:?}")),
    };
    if let Err(error) = result {
        eprintln!("fixture: {error}");
        std::process::exit(1);
    }
}

fn beside() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    executable
        .parent()
        .map(PathBuf::from)
        .ok_or_else(|| "the executable has no folder".to_owned())
}

fn helper() -> Result<(), String> {
    let name = if cfg!(windows) {
        "fixture-helper.exe"
    } else {
        "fixture-helper"
    };
    let output = std::process::Command::new(beside()?.join(name))
        .output()
        .map_err(|e| format!("{name}: {e}"))?;
    if !output.status.success() {
        return Err(format!("{name} failed: {}", output.status));
    }
    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

fn update() -> Result<(), String> {
    let staging = std::env::var_os("FIXTURE_STAGING")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("roadcase-fixture-updates"));
    let config = Config::new(FEED, PUBLIC_KEY, VERSION, staging).map_err(|e| e.to_string())?;
    let updater = Updater::new(config).map_err(|e| e.to_string())?;
    let Some(update) = updater.check().map_err(|e| e.to_string())? else {
        println!("up to date");
        return Ok(());
    };
    println!("available {}", update.version);
    let staged = updater
        .download(&update, |_, _| {})
        .map_err(|e| e.to_string())?;
    staged.install(&["relaunched"]).map_err(|e| e.to_string())?;
    println!("installed, leaving");
    std::process::exit(0);
}

fn relaunched() -> Result<(), String> {
    use std::io::Write;
    let log = match std::env::var_os("FIXTURE_LOG") {
        Some(log) => PathBuf::from(log),
        // <folder>/<Name>.app/Contents/MacOS/fixture: beside the bundle.
        None => beside()?
            .ancestors()
            .nth(3)
            .ok_or("the application is not in a bundle, and $FIXTURE_LOG is not set")?
            .join("fixture.log"),
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .map_err(|e| e.to_string())?;
    writeln!(file, "{VERSION}").map_err(|e| e.to_string())
}
