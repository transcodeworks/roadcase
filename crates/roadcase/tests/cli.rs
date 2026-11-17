//! The `roadcase` binary, end to end, offline.

use std::path::Path;
use std::process::{Command, Output};

fn roadcase(arguments: &[&str], cwd: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_roadcase"))
        .args(arguments)
        .current_dir(cwd)
        .env_remove("ROADCASE_UPDATE_SECRET_KEY")
        .env_remove("CC_UPDATE_SECRET_KEY")
        .env_remove("ROADCASE_UPDATE_KEY_PASSWORD")
        .output()
        .unwrap()
}

fn roadcase_with(arguments: &[&str], cwd: &Path, environment: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_roadcase"));
    command
        .args(arguments)
        .current_dir(cwd)
        .env_remove("ROADCASE_UPDATE_SECRET_KEY")
        .env_remove("CC_UPDATE_SECRET_KEY")
        .env_remove("ROADCASE_UPDATE_KEY_PASSWORD");
    for (name, value) in environment {
        command.env(name, value);
    }
    command.output().unwrap()
}

fn stderr(output: &Output) -> String {
    assert!(!output.status.success());
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_key_with_a_password() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // Without a terminal, the password comes from the environment.
    assert!(
        stderr(&roadcase(&["keygen", "--password", "keys"], root))
            .contains("ROADCASE_UPDATE_KEY_PASSWORD")
    );
    assert!(!root.join("keys").exists());
    let password = [("ROADCASE_UPDATE_KEY_PASSWORD", "correct horse")];
    stdout(&roadcase_with(
        &["keygen", "--password", "keys"],
        root,
        &password,
    ));
    let secret = std::fs::read_to_string(root.join("keys/update.key")).unwrap();
    assert!(secret.lines().nth(1).unwrap().starts_with("RWRTY0Iy"));

    std::fs::write(root.join("App.AppImage"), b"appimage").unwrap();
    let publish = [
        "publish",
        "--version",
        "1.0.0",
        "--out",
        "feed",
        "--key",
        "keys/update.key",
        "--artifact",
        "linux-x86_64=appimage=App.AppImage",
    ];
    assert!(stderr(&roadcase(&publish, root)).contains("--password-env"));
    let wrong = [("ROADCASE_UPDATE_KEY_PASSWORD", "wrong horse")];
    assert!(stderr(&roadcase_with(&publish, root, &wrong)).contains("wrong password"));
    assert!(!root.join("feed").exists());

    stdout(&roadcase_with(&publish, root, &password));
    let named = [&publish[..], &["--password-env", "RELEASE_KEY_PASSWORD"]].concat();
    assert!(stderr(&roadcase(&named, root)).contains("RELEASE_KEY_PASSWORD is not set"));
    stdout(&roadcase_with(
        &named,
        root,
        &[("RELEASE_KEY_PASSWORD", "correct horse"), wrong[0]],
    ));
    stdout(&roadcase(
        &["verify", "--key", "keys/update.pub", "feed"],
        root,
    ));
}

fn stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn keygen_publish_and_verify() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let public = stdout(&roadcase(&["keygen", "keys"], root));
    assert!(public.starts_with("RW"));
    // A key is never replaced.
    assert!(!roadcase(&["keygen", "keys"], root).status.success());

    std::fs::write(root.join("App 1.0.0 setup.exe"), b"installer").unwrap();
    let published = stdout(&roadcase(
        &[
            "publish",
            "--version",
            "1.0.0",
            "--out",
            "feed",
            "--key",
            "keys/update.key",
            "--notes",
            "First.",
            "--artifact",
            "windows-x86_64=nsis=App 1.0.0 setup.exe",
        ],
        root,
    ));
    assert!(published.contains("1.0.0 for windows-x86_64"));
    let verified = stdout(&roadcase(
        &["verify", "--key", "keys/update.pub", "feed"],
        root,
    ));
    assert!(verified.contains("windows-x86_64: App 1.0.0 setup.exe matches"));

    // The secret key from the environment, under its old name too.
    let secret = std::fs::read_to_string(root.join("keys/update.key")).unwrap();
    std::fs::write(root.join("App.app.tar.gz"), b"archive").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_roadcase"))
        .args([
            "publish",
            "--version",
            "1.0.0",
            "--out",
            "feed",
            "--artifact",
            "macos-aarch64=app-tar-gz=App.app.tar.gz",
        ])
        .current_dir(root)
        .env_remove("ROADCASE_UPDATE_SECRET_KEY")
        .env("CC_UPDATE_SECRET_KEY", secret)
        .output()
        .unwrap();
    assert!(stdout(&output).contains("macos-aarch64, windows-x86_64"));

    // Without any key, it says where it looked.
    let refused = roadcase(
        &[
            "publish",
            "--version",
            "1.0.0",
            "--out",
            "feed",
            "--artifact",
            "macos-aarch64=app-tar-gz=App.app.tar.gz",
        ],
        root,
    );
    assert!(String::from_utf8_lossy(&refused.stderr).contains("ROADCASE_UPDATE_SECRET_KEY"));

    // A changed artifact is caught.
    std::fs::write(root.join("feed/App 1.0.0 setup.exe"), b"changed").unwrap();
    assert!(
        !roadcase(&["verify", "--key", "keys/update.pub", "feed"], root)
            .status
            .success()
    );
}

const CONFIG: &str = r#"
[app]
name = "Tiny"
id = "com.example.tiny"
version = "0.1.0"

[macos.executable]
from = "bin/tiny"
"#;

#[test]
fn check_app_and_release() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("roadcase.toml"), CONFIG).unwrap();
    std::fs::create_dir(root.join("bin")).unwrap();
    std::fs::write(root.join("bin/tiny"), b"tiny").unwrap();

    let checked = stdout(&roadcase(&["check", "--system", "macos"], root));
    assert!(checked.contains("Tiny (com.example.tiny)"));
    assert!(checked.contains("version 0.1.0"));

    let app = stdout(&roadcase(
        &["app", "--system", "macos", "--out", "out"],
        root,
    ));
    assert_eq!(
        app.trim(),
        Path::new("out").join("Tiny.app").display().to_string()
    );
    assert!(root.join("out/Tiny.app/Contents/MacOS/Tiny").is_file());

    // A release is made on a Mac, with the system's tools: scripts/macos-e2e.sh
    // goes through one. Elsewhere it says so.
    if !cfg!(target_os = "macos") {
        let release = roadcase(&["release", "macos", "--out", "out"], root);
        assert!(!release.status.success());
        assert!(String::from_utf8_lossy(&release.stderr).contains("never cross-built"));
        let keychain = roadcase(&["keychain", "import"], root);
        assert!(!keychain.status.success());
    }
}

#[test]
fn a_package_is_published_at_its_repository_url() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    stdout(&roadcase(&["keygen", "keys"], root));
    std::fs::write(root.join("tiny_0.1.0-1_amd64.deb"), b"deb").unwrap();
    let url = "https://packages.example.com/apt/pool/main/t/tiny/tiny_0.1.0-1_amd64.deb";
    let publish = |extra: &[&str]| {
        let mut arguments = vec![
            "publish",
            "--version",
            "0.1.0",
            "--out",
            "feed",
            "--key",
            "keys/update.key",
            "--artifact",
            "linux-x86_64-deb=deb=tiny_0.1.0-1_amd64.deb",
        ];
        arguments.extend(extra);
        roadcase(&arguments, root)
    };
    let wrong = publish(&["--url", "linux-aarch64-deb=https://example.com/x.deb"]);
    assert!(!wrong.status.success());
    stdout(&publish(&["--url", &format!("linux-x86_64-deb={url}")]));
    let manifest = std::fs::read_to_string(root.join("feed/latest.json")).unwrap();
    assert!(
        manifest.contains(&format!("\"url\": \"{url}\"")),
        "{manifest}"
    );
    assert!(!root.join("feed/tiny_0.1.0-1_amd64.deb").exists());
}

const LINUX: &str = r#"
[app]
name = "Tiny"
id = "com.example.tiny"
version = "0.1.0"
url_schemes = ["tiny"]

[linux]
icons = ["icon.svg"]
categories = ["Utility"]

[linux.executable]
from = "bin/tiny"
"#;

#[test]
fn linux_check_app_release_and_publish() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("roadcase.toml"), LINUX).unwrap();
    std::fs::create_dir(root.join("bin")).unwrap();
    std::fs::write(root.join("bin/tiny"), b"tiny").unwrap();
    std::fs::write(root.join("icon.svg"), b"<svg/>").unwrap();

    let checked = stdout(&roadcase(&["check", "--system", "linux"], root));
    assert!(checked.contains("Linux: usr/bin/tiny and 0 program(s), desktop entry com.example.tiny.desktop for x-scheme-handler/tiny"), "{checked}");

    let app = stdout(&roadcase(
        &["app", "--system", "linux", "--out", "out"],
        root,
    ));
    assert_eq!(
        app.trim(),
        Path::new("out").join("Tiny.AppDir").display().to_string()
    );
    assert!(
        root.join("out/Tiny.AppDir/com.example.tiny.desktop")
            .is_file()
    );

    let release = roadcase(&["release", "linux", "--out", "out", "--skip-build"], root);
    if cfg!(target_os = "linux") {
        stdout(&release);
    } else {
        assert!(!release.status.success());
        assert!(String::from_utf8_lossy(&release.stderr).contains("never cross-built"));
    }

    stdout(&roadcase(&["keygen", "keys"], root));
    std::fs::write(root.join("Tiny-0.1.0-x86_64.AppImage"), b"appimage").unwrap();
    let published = stdout(&roadcase(
        &[
            "publish",
            "--version",
            "0.1.0",
            "--out",
            "feed",
            "--key",
            "keys/update.key",
            "--artifact",
            "linux-x86_64=appimage=Tiny-0.1.0-x86_64.AppImage",
        ],
        root,
    ));
    assert!(published.contains("0.1.0 for linux-x86_64"));
    let manifest = std::fs::read_to_string(root.join("feed/latest.json")).unwrap();
    assert!(manifest.contains("\"kind\": \"appimage\""), "{manifest}");
    let verified = stdout(&roadcase(
        &["verify", "--key", "keys/update.pub", "feed"],
        root,
    ));
    assert!(verified.contains("linux-x86_64: Tiny-0.1.0-x86_64.AppImage matches"));
}

#[test]
fn a_repository_key_is_made_once_and_signs_nothing_without_packages() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let made = roadcase(
        &[
            "repo",
            "keygen",
            "keys",
            "--name",
            "Example Packages <packages@example.com>",
        ],
        root,
    );
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    let fingerprint = String::from_utf8_lossy(&made.stdout).trim().to_owned();
    assert_eq!(fingerprint.len(), 40);
    let secret = std::fs::read_to_string(root.join("keys/signing.key")).unwrap();
    assert!(secret.starts_with("-----BEGIN PGP PRIVATE KEY BLOCK-----"));
    assert!(
        std::fs::read_to_string(root.join("keys/key.asc"))
            .unwrap()
            .starts_with("-----BEGIN PGP PUBLIC KEY BLOCK-----")
    );
    assert!(root.join("keys/key.gpg").is_file());
    assert!(
        stderr(&roadcase(
            &["repo", "keygen", "keys", "--name", "Again <a@example.com>"],
            root
        ))
        .contains("exists")
    );

    // No key, no repository.
    assert!(stderr(&roadcase(&["repo", "apt", "apt"], root)).contains("ROADCASE_REPO_SIGNING_KEY"));
    // A key, and nothing to list.
    let refused = roadcase(
        &[
            "repo",
            "pacman",
            "arch",
            "--name",
            "example",
            "--key",
            "keys/signing.key",
        ],
        root,
    );
    assert!(stderr(&refused).contains("holds no package"));
    let refused = roadcase_with(
        &["repo", "apt", "apt"],
        root,
        &[("ROADCASE_REPO_SIGNING_KEY", &secret)],
    );
    assert!(stderr(&refused).contains("holds no .deb"));
}

#[test]
fn a_deb_is_announced_under_its_own_key() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    stdout(&roadcase(&["keygen", "keys"], root));
    std::fs::write(root.join("app_1.0.0-1_amd64.deb"), b"deb").unwrap();
    let publish = |artifact: &str| {
        roadcase(
            &[
                "publish",
                "--version",
                "1.0.0",
                "--out",
                "feed",
                "--key",
                "keys/update.key",
                "--artifact",
                artifact,
            ],
            root,
        )
    };
    assert!(stderr(&publish("linux-x86_64=deb=app_1.0.0-1_amd64.deb")).contains("<target>-deb"));
    let published = publish("linux-x86_64-deb=deb=app_1.0.0-1_amd64.deb");
    assert!(
        published.status.success(),
        "{}",
        String::from_utf8_lossy(&published.stderr)
    );
    let manifest = std::fs::read_to_string(root.join("feed/latest.json")).unwrap();
    assert!(
        manifest.contains("\"linux-x86_64-deb\"") && manifest.contains("\"kind\": \"deb\""),
        "{manifest}"
    );
}
