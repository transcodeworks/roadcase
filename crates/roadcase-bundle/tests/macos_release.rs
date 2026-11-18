//! A whole macOS release, signed ad hoc, with the system's own tools
//! (`codesign`, `hdiutil`, `tar`): on a Mac only. The application's
//! executable is this test's own, a real Mach-O program. Symbols, which need
//! Xcode's command-line tools, and notarization are left to
//! scripts/macos-e2e.sh.
#![cfg(target_os = "macos")]

use roadcase_bundle::{Environment, Host, Options, Project, macos, publish};
use std::collections::BTreeMap;
use std::process::Command;

const CONFIG: &str = r#"
[app]
name = "Tiny App"
id = "com.example.tiny"
version = "0.3.0"

[macos]
entitlements = "app.entitlements"
[macos.executable]
from = "bin/tiny"

[[macos.program]]
name = "tiny-helper"
from = "bin/tiny"
entitlements = "helper.entitlements"
identifier = "{id}.helper"

[[macos.resource]]
from = "sparse-resource"
to = "Resources"

[macos.dmg]
volume_name = "Tiny"
[macos.update_archive]
# Read, and skipped: an ad hoc signature is never notarized.
[macos.notarize]

[feed]
url = "https://updates.example.com/tiny/"
public_key = { env = "PUBLIC_KEY" }
"#;

fn entitlements(key: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>{key}</key><true/></dict></plist>\n"
    )
}

fn said(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn an_ad_hoc_release_signs_images_archives_and_publishes() {
    let keys = roadcase_update::sign::generate().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("bin")).unwrap();
    std::fs::copy(std::env::current_exe().unwrap(), root.join("bin/tiny")).unwrap();
    std::fs::write(
        root.join("app.entitlements"),
        entitlements("com.apple.security.network.client"),
    )
    .unwrap();
    std::fs::write(
        root.join("helper.entitlements"),
        entitlements("com.apple.security.cs.disable-library-validation"),
    )
    .unwrap();
    // The image must fit logical file sizes even when the source is sparse.
    std::fs::File::create(root.join("sparse-resource"))
        .unwrap()
        .set_len(80 * 1024 * 1024)
        .unwrap();
    let options = Options {
        variant: None,
        host: Host::macos(),
        environment: Environment::Fixed(BTreeMap::from([(
            "ROADCASE_UPDATE_SECRET_KEY".to_owned(),
            keys.secret.clone(),
        )])),
    };
    let mut project = Project::parse(CONFIG, root, options).unwrap();
    // No CODESIGN_IDENTITY, no certificate: ad hoc, with no team.
    let signing = macos::Signing::prepare(&mut project).unwrap();
    assert!(signing.identity.is_ad_hoc());

    let out = root.join("out");
    let release = macos::release(&project, &signing.identity, &out).unwrap();
    signing.finish().unwrap();
    assert!(!release.notarized);
    assert!(release.symbols.is_none());
    let app = &release.app.path;

    said(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(app),
    );
    let signature = said(
        Command::new("/usr/bin/codesign")
            .args(["--display", "--verbose=2"])
            .arg(app),
    );
    assert!(signature.contains("Signature=adhoc"), "{signature}");
    assert!(
        signature.contains("Identifier=com.example.tiny\n"),
        "{signature}"
    );
    let helper = app.join("Contents/MacOS/tiny-helper");
    let signature = said(
        Command::new("/usr/bin/codesign")
            .args(["--display", "--verbose=2"])
            .arg(&helper),
    );
    assert!(
        signature.contains("Identifier=com.example.tiny.helper\n"),
        "{signature}"
    );
    let granted = |path: &std::path::Path| {
        said(
            Command::new("/usr/bin/codesign")
                .args(["--display", "--entitlements", "-", "--xml"])
                .arg(path),
        )
    };
    assert!(granted(&helper).contains("disable-library-validation"));
    assert!(!granted(&helper).contains("network.client"));
    assert!(granted(app).contains("network.client"));
    assert!(!granted(app).contains("disable-library-validation"));

    let dmg = release.dmg.unwrap();
    assert_eq!(
        dmg,
        out.join(format!("Tiny App-0.3.0-{}.dmg", std::env::consts::ARCH))
    );
    said(Command::new("/usr/bin/codesign").arg("--verify").arg(&dmg));
    said(Command::new("/usr/bin/hdiutil").arg("verify").arg(&dmg));

    let mount = root.join("mounted");
    said(
        Command::new("/usr/bin/hdiutil")
            .args(["attach", "-readonly", "-nobrowse", "-mountpoint"])
            .arg(&mount)
            .arg(&dmg),
    );
    let resource = std::fs::metadata(mount.join("Tiny App.app/Contents/Resources/sparse-resource"));
    let link = std::fs::read_link(mount.join("Applications"));
    let signature = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(mount.join("Tiny App.app"))
        .output();
    // Detach before asserting so a failure does not leave a mounted test image.
    said(Command::new("/usr/bin/hdiutil").arg("detach").arg(&mount));
    assert_eq!(resource.unwrap().len(), 80 * 1024 * 1024);
    assert_eq!(link.unwrap(), std::path::Path::new("/Applications"));
    assert!(signature.unwrap().status.success());

    // The archive holds the signed bundle, and no ._ files.
    let archive = release.update_archive.unwrap();
    let listing = said(Command::new("/usr/bin/tar").arg("-tzf").arg(&archive));
    assert!(listing.contains("Tiny App.app/Contents/_CodeSignature/CodeResources"));
    assert!(
        !listing.lines().any(|line| line
            .rsplit('/')
            .next()
            .is_some_and(|name| name.starts_with("._"))),
        "{listing}"
    );

    let part = publish::macos_part(&project, &archive);
    let published = publish::publish(&project, &[part]).unwrap().unwrap();
    assert_eq!(published.folder, root.join("target/feed"));
    let target = format!("macos-{}", std::env::consts::ARCH);
    assert_eq!(
        published.manifest.artifacts[&target].url,
        format!("Tiny%20App-0.3.0-{}.app.tar.gz", std::env::consts::ARCH)
    );
    assert!(published.folder.join("latest.json.minisig").is_file());
}
