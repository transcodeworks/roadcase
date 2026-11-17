//! A tiny application for scripts/linux-e2e.sh: its version, feed and key
//! are compiled in from FIXTURE_VERSION, FIXTURE_FEED and FIXTURE_PUBLIC_KEY.
//!
//! - `version` prints the version;
//! - `update` checks the feed, downloads and installs a newer release, then
//!   leaves, asking to be started again with `relaunched`;
//! - `relaunched` appends its version and `$APPIMAGE` to `$FIXTURE_LOG`;
//! - `desktop-install` and `desktop-remove` call `roadcase_update::desktop`;
//! - `installed-by` prints the package manager that installed it, or `none`;
//! - `check` prints the newer release the feed announces, and which package
//!   manager installs it, then asks to download it, which a packaged copy
//!   must refuse with `ManagedBySystem`.

use roadcase_update::{Config, Updater};

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
        "update" => update(),
        "relaunched" => relaunched(),
        "installed-by" => {
            match roadcase_update::package_manager() {
                Some(manager) => println!("{}", manager.marker()),
                None => println!("none"),
            }
            Ok(())
        }
        "check" => check(),
        "desktop-install" => roadcase_update::desktop::install()
            .map(|changed| println!("changed={changed}"))
            .map_err(|error| error.to_string()),
        "desktop-remove" => roadcase_update::desktop::remove()
            .map(|removed| println!("removed={removed}"))
            .map_err(|error| error.to_string()),
        other => Err(format!("unknown command {other:?}")),
    };
    if let Err(error) = result {
        eprintln!("roadcase-fixture: {error}");
        std::process::exit(1);
    }
}

fn update() -> Result<(), String> {
    let home = std::env::var_os("HOME").ok_or("no $HOME")?;
    let staging = std::path::Path::new(&home).join(".cache/roadcase-fixture/updates");
    let config = Config::new(FEED, PUBLIC_KEY, VERSION, staging).map_err(|e| e.to_string())?;
    let updater = Updater::new(config).map_err(|e| e.to_string())?;
    let Some(update) = updater.check().map_err(|e| e.to_string())? else {
        println!("up to date");
        return Ok(());
    };
    let staged = updater
        .download(&update, |_, _| {})
        .map_err(|e| e.to_string())?;
    staged.install(&["relaunched"]).map_err(|e| e.to_string())?;
    println!("installed, leaving");
    std::process::exit(0);
}

fn check() -> Result<(), String> {
    let home = std::env::var_os("HOME").ok_or("no $HOME")?;
    let staging = std::path::Path::new(&home).join(".cache/roadcase-fixture/updates");
    let config = Config::new(FEED, PUBLIC_KEY, VERSION, staging).map_err(|e| e.to_string())?;
    let updater = Updater::new(config).map_err(|e| e.to_string())?;
    let Some(update) = updater.check().map_err(|e| e.to_string())? else {
        println!("up to date");
        return Ok(());
    };
    let manager = update
        .managed_by()
        .map_or("none", |manager| manager.marker());
    println!("available {} managed-by={manager}", update.version);
    match updater.download(&update, |_, _| {}) {
        Err(roadcase_update::Error::ManagedBySystem { manager }) => {
            println!("download refused: ManagedBySystem {manager}");
            Ok(())
        }
        Err(error) => Err(error.to_string()),
        Ok(_) => Err("a packaged copy downloaded its update".into()),
    }
}

fn relaunched() -> Result<(), String> {
    use std::io::Write;
    let log = std::env::var_os("FIXTURE_LOG").ok_or("no $FIXTURE_LOG")?;
    let appimage = std::env::var("APPIMAGE").unwrap_or_default();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .map_err(|e| e.to_string())?;
    writeln!(file, "{VERSION} {appimage}").map_err(|e| e.to_string())
}
