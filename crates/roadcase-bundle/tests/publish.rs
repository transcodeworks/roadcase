//! Publishing from the config: each system's part of the feed, signed, with
//! packages announced at their repository's URL and never copied.

use roadcase_bundle::{Environment, Host, Options, Project, publish};
use roadcase_update::{Kind, MANIFEST, Manifest, PublicKey, SIGNATURE};
use std::collections::BTreeMap;
use std::path::Path;

const CONFIG: &str = r#"
[app]
name = "Example"
id = "com.example.app"
version = "1.2.0+build.7"

[macos.executable]
from = "bin/example"
[macos.update_archive]

[windows.executable]
from = "bin/example.exe"
[windows.nsis]
script = "installer.nsi"

[linux]
icons = ["icon.svg"]
categories = ["Utility"]
[linux.executable]
from = "bin/example"
[linux.appimage]
[linux.deb]
maintainer = "Example <packages@example.com>"
[linux.deb.apt_source]
url = "https://packages.example.com/apt/"
key = "key.gpg"
[linux.pacman]
repository = "https://packages.example.com/arch"

[feed]
url = "https://updates.example.com/{channel}/"
public_key = { env = "PUBLIC_KEY" }
out = "feed/{channel}"
notes = { env = "NOTES", default = "" }
"#;

fn project(dir: &Path, host: Host, env: &[(&str, &str)]) -> Project {
    let options = Options {
        variant: None,
        host,
        environment: Environment::Fixed(
            env.iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect::<BTreeMap<_, _>>(),
        ),
    };
    Project::parse(CONFIG, dir, options).unwrap()
}

fn linux() -> Host {
    Host {
        arch: "x86_64".into(),
        triple: "x86_64-unknown-linux-gnu".into(),
    }
}

fn mac() -> Host {
    Host {
        arch: "aarch64".into(),
        triple: "aarch64-apple-darwin".into(),
    }
}

fn verified(folder: &Path, public: &str) -> Manifest {
    Manifest::verified(
        &std::fs::read(folder.join(MANIFEST)).unwrap(),
        &std::fs::read_to_string(folder.join(SIGNATURE)).unwrap(),
        &PublicKey::parse(public).unwrap(),
    )
    .unwrap()
}

#[test]
fn each_system_adds_its_part_to_one_signed_feed() {
    let keys = roadcase_update::sign::generate().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let env = [
        ("ROADCASE_UPDATE_SECRET_KEY", keys.secret.as_str()),
        ("NOTES", "Faster."),
    ];
    let file = |name: &str| {
        let path = root.join(name);
        std::fs::write(&path, name.as_bytes()).unwrap();
        path
    };

    // macOS: the update archive (never the disk image).
    let project = project(root, mac(), &env);
    let part = publish::macos_part(&project, &file("Example-1.2.0-aarch64.app.tar.gz"));
    let published = publish::publish(&project, &[part]).unwrap().unwrap();
    assert_eq!(published.folder, root.join("feed/stable"));
    assert_eq!(published.manifest.notes.as_deref(), Some("Faster."));

    // Windows: the installer, under its versioned feed name.
    let project = self::project(root, Host::windows(), &env);
    let part = publish::windows_part(&project, &file("Example-Setup.exe")).unwrap();
    assert!(part.path.ends_with("Example-1.2.0+build.7-x64-setup.exe"));
    publish::publish(&project, &[part]).unwrap().unwrap();

    // Linux: the AppImage in the feed, the packages where their repositories
    // keep them.
    let project = self::project(root, linux(), &env);
    let deb = file("example_1.2.0+build.7-1_amd64.deb");
    let package = file("example-1.2.0-1-x86_64.pkg.tar.zst");
    let parts = [
        publish::appimage_part(&project, &file("Example-1.2.0-x86_64.AppImage")),
        publish::deb_part(&project, &deb).unwrap().unwrap(),
        publish::pacman_part(&project, &package).unwrap().unwrap(),
    ];
    let published = publish::publish(&project, &parts).unwrap().unwrap();

    let folder = &published.folder;
    let manifest = verified(folder, &keys.public);
    assert_eq!(manifest, published.manifest);
    assert_eq!(manifest.version.to_string(), "1.2.0+build.7");
    let kinds: Vec<_> = manifest
        .artifacts
        .iter()
        .map(|(target, artifact)| (target.as_str(), artifact.kind, artifact.url.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            (
                "linux-x86_64",
                Kind::AppImage,
                "Example-1.2.0-x86_64.AppImage"
            ),
            (
                "linux-x86_64-deb",
                Kind::Deb,
                "https://packages.example.com/apt/pool/main/e/example/example_1.2.0%2Bbuild.7-1_amd64.deb"
            ),
            (
                "linux-x86_64-pacman",
                Kind::Pacman,
                "https://packages.example.com/arch/x86_64/example-1.2.0-1-x86_64.pkg.tar.zst"
            ),
            (
                "macos-aarch64",
                Kind::AppTarGz,
                "Example-1.2.0-aarch64.app.tar.gz"
            ),
            (
                "windows-x86_64",
                Kind::Nsis,
                "Example-1.2.0%2Bbuild.7-x64-setup.exe"
            ),
        ]
    );
    // What installed copies download is in the folder; packages are not.
    for name in [
        "Example-1.2.0-aarch64.app.tar.gz",
        "Example-1.2.0+build.7-x64-setup.exe",
        "Example-1.2.0-x86_64.AppImage",
    ] {
        assert!(folder.join(name).is_file(), "{name}");
    }
    assert!(!folder.join(deb.file_name().unwrap()).exists());
    assert!(!folder.join(package.file_name().unwrap()).exists());
    let announced = &manifest.artifacts["linux-x86_64-deb"];
    assert_eq!(
        (announced.sha256.clone(), announced.size),
        roadcase_update::sign::digest(&deb).unwrap()
    );
}

#[test]
fn without_a_key_nothing_is_published() {
    let dir = tempfile::tempdir().unwrap();
    let project = project(dir.path(), mac(), &[]);
    let archive = dir.path().join("Example.app.tar.gz");
    std::fs::write(&archive, b"archive").unwrap();
    let part = publish::macos_part(&project, &archive);
    assert!(publish::secret_key(&project).unwrap().is_none());
    assert!(publish::publish(&project, &[part]).unwrap().is_none());
    assert!(!dir.path().join("feed").exists());
}

#[test]
fn a_key_with_a_password_needs_it() {
    let keys = roadcase_update::sign::generate_with_password("open sesame").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("Example.app.tar.gz");
    std::fs::write(&archive, b"archive").unwrap();
    // The older name of the variable is read too.
    let locked = project(
        dir.path(),
        mac(),
        &[("CC_UPDATE_SECRET_KEY", keys.secret.as_str())],
    );
    let part = publish::macos_part(&locked, &archive);
    if std::env::var_os(roadcase_update::sign::PASSWORD_ENV).is_none() {
        let error = publish::publish(&locked, std::slice::from_ref(&part)).unwrap_err();
        assert!(error.to_string().contains("password"), "{error}");
        assert!(!dir.path().join("feed").exists());
    }
    let opened = project(
        dir.path(),
        mac(),
        &[
            ("CC_UPDATE_SECRET_KEY", keys.secret.as_str()),
            ("ROADCASE_UPDATE_KEY_PASSWORD", "open sesame"),
        ],
    );
    let published = publish::publish(&opened, &[part]).unwrap().unwrap();
    verified(&published.folder, &keys.public);
}

#[test]
fn packages_without_a_repository_are_not_announced() {
    let config = CONFIG
        .replace(
            "[linux.deb.apt_source]\nurl = \"https://packages.example.com/apt/\"\nkey = \"key.gpg\"\n",
            "",
        )
        .replace("repository = \"https://packages.example.com/arch\"\n", "");
    let options = Options {
        variant: None,
        host: linux(),
        environment: Environment::Fixed(BTreeMap::new()),
    };
    let project = Project::parse(&config, Path::new("."), options).unwrap();
    assert!(
        publish::deb_part(&project, Path::new("example_1.2.0-1_amd64.deb"))
            .unwrap()
            .is_none()
    );
    assert!(
        publish::pacman_part(&project, Path::new("example-1.2.0-1-x86_64.pkg.tar.zst"))
            .unwrap()
            .is_none()
    );
}
