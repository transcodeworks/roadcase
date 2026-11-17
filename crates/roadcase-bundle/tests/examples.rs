//! The repository's example configs parse, validate, and assemble into the
//! bundles they describe, from fixture files in a temporary folder. No signing
//! identity, Apple tool or network is needed.

use plist::Value;
use roadcase_bundle::{Environment, Error, Host, Options, Project, info, macos};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
        .join("roadcase.toml")
}

fn options(variant: Option<&str>, env: &[(&str, &str)]) -> Options {
    Options {
        variant: variant.map(str::to_owned),
        host: Host {
            arch: "aarch64".into(),
            triple: "aarch64-apple-darwin".into(),
        },
        environment: Environment::Fixed(
            env.iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect::<BTreeMap<_, _>>(),
        ),
    }
}

const PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
BODY
</dict>
</plist>
"#;

fn plist_file(body: &str) -> String {
    PLIST.replace("BODY", body)
}

/// Copies an example into a temporary folder with these files beside it.
fn stage(name: &str, files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(example(name), dir.path().join("roadcase.toml")).unwrap();
    for (path, content) in files {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    dir
}

fn read(path: &Path) -> plist::Dictionary {
    Value::from_file(path).unwrap().into_dictionary().unwrap()
}

fn string<'a>(dictionary: &'a plist::Dictionary, key: &str) -> &'a str {
    dictionary
        .get(key)
        .and_then(Value::as_string)
        .unwrap_or_else(|| panic!("{key} is not a string"))
}

const CARGO: &str = "[package]\nname = \"x\"\nversion = \"2.3.0-beta.1\"\n";

fn helper_files() -> Vec<(&'static str, String)> {
    vec![
        ("Cargo.toml", CARGO.into()),
        ("target/release/example-studio", "main".into()),
        // The triple's build is empty (a placeholder): the universal one wins.
        (
            "binaries/example-plugin-host-aarch64-apple-darwin",
            String::new(),
        ),
        (
            "binaries/example-plugin-host-universal-apple-darwin",
            "host".into(),
        ),
        (
            "binaries/example-ffmpeg-aarch64-apple-darwin",
            "ffmpeg".into(),
        ),
        ("docs/guide.md", "# Guide".into()),
        ("docs/cues.md", "# Cues".into()),
        ("docs/notes.txt", "not a manual page".into()),
        ("icons/icon.icns", "icns".into()),
        ("icons/Assets.car", "car".into()),
        (
            "packaging/macos/app.entitlements",
            plist_file("<key>com.apple.security.device.camera</key><true/>"),
        ),
        (
            "packaging/macos/plugin-host.entitlements",
            plist_file("<key>com.apple.security.cs.disable-library-validation</key><true/>"),
        ),
    ]
}

fn staged(name: &str, files: &[(&'static str, String)]) -> tempfile::TempDir {
    let borrowed: Vec<(&str, &str)> = files.iter().map(|(p, c)| (*p, c.as_str())).collect();
    stage(name, &borrowed)
}

#[test]
fn the_helper_app_assembles() {
    let dir = staged("helper-app", &helper_files());
    let project = Project::load(&dir.path().join("roadcase.toml"), options(None, &[])).unwrap();
    let out = dir.path().join("out");
    let app = macos::assemble(&project, &out).unwrap();

    assert_eq!(app.path, out.join("Example Studio.app"));
    let contents = app.path.join("Contents");
    let file = |path: &str| std::fs::read_to_string(contents.join(path)).unwrap();
    assert_eq!(file("MacOS/Example Studio"), "main");
    assert_eq!(file("MacOS/example-plugin-host"), "host");
    assert_eq!(file("MacOS/example-ffmpeg"), "ffmpeg");
    assert_eq!(file("Resources/resources/docs/guide.md"), "# Guide");
    assert!(contents.join("Resources/resources/docs/cues.md").is_file());
    assert!(!contents.join("Resources/resources/docs/notes.txt").exists());
    assert_eq!(file("Resources/Example Studio.icns"), "icns");
    assert_eq!(file("Resources/Assets.car"), "car");
    assert_eq!(file("PkgInfo"), "APPL????");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(contents.join("MacOS/example-ffmpeg"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    let plist = read(&contents.join("Info.plist"));
    assert_eq!(string(&plist, "CFBundleIdentifier"), "com.example.studio");
    assert_eq!(string(&plist, "CFBundleExecutable"), "Example Studio");
    assert_eq!(string(&plist, "CFBundleName"), "Example Studio");
    assert_eq!(string(&plist, "CFBundlePackageType"), "APPL");
    assert_eq!(string(&plist, "CFBundleShortVersionString"), "2.3.0");
    assert_eq!(string(&plist, "CFBundleVersion"), "2.3.0-beta.1");
    assert_eq!(string(&plist, "LSMinimumSystemVersion"), "11.0");
    assert_eq!(string(&plist, "CFBundleIconFile"), "Example Studio");
    assert_eq!(string(&plist, "CFBundleIconName"), "AppIcon");
    assert_eq!(
        string(&plist, "NSCameraUsageDescription"),
        "Example Studio shows live video from the cameras you choose."
    );
    let url_types = plist["CFBundleURLTypes"].as_array().unwrap();
    let url_type = url_types[0].as_dictionary().unwrap();
    assert_eq!(
        url_type["CFBundleURLSchemes"].as_array().unwrap()[0].as_string(),
        Some("examplestudio")
    );
    let documents = plist["CFBundleDocumentTypes"].as_array().unwrap();
    assert_eq!(documents.len(), 2);
    let venue = documents[1].as_dictionary().unwrap();
    assert_eq!(string(venue, "CFBundleTypeRole"), "Viewer");
    assert_eq!(string(venue, "LSHandlerRank"), "Owner");
    assert_eq!(
        plist["UTExportedTypeDeclarations"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(!plist.contains_key("UTImportedTypeDeclarations"));

    // Inside out: the two programs, then the bundle; the plug-in host with
    // its own entitlements.
    let order: Vec<_> = app
        .signing
        .iter()
        .map(|step| {
            step.path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        order,
        [
            "example-plugin-host",
            "example-ffmpeg",
            "Example Studio.app"
        ]
    );
    assert!(
        app.signing[0]
            .entitlements
            .as_ref()
            .unwrap()
            .ends_with("plugin-host.entitlements")
    );
    assert_eq!(app.signing[1].entitlements, None);
    assert!(
        app.signing[2]
            .entitlements
            .as_ref()
            .unwrap()
            .ends_with("app.entitlements")
    );
    assert_eq!(
        app.symbols,
        [
            contents.join("MacOS/example-plugin-host"),
            contents.join("MacOS/Example Studio")
        ]
    );

    // Assembling again replaces the bundle.
    std::fs::write(contents.join("stale"), "x").unwrap();
    macos::assemble(&project, &out).unwrap();
    assert!(!contents.join("stale").exists());
}

#[test]
fn the_test_variant_claims_nothing() {
    let dir = staged("helper-app", &helper_files());
    let path = dir.path().join("roadcase.toml");
    let project =
        Project::load(&path, options(Some("test"), &[("STUDIO_VERSION", "3.0.0")])).unwrap();
    let plist = info::app(&project).unwrap();
    assert_eq!(
        string(&plist, "CFBundleIdentifier"),
        "com.example.studio.test"
    );
    assert_eq!(string(&plist, "CFBundleShortVersionString"), "3.0.0");
    assert!(!plist.contains_key("CFBundleURLTypes"));
    assert!(!plist.contains_key("UTExportedTypeDeclarations"));
    assert_eq!(
        plist["UTImportedTypeDeclarations"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    for document in plist["CFBundleDocumentTypes"].as_array().unwrap() {
        assert_eq!(
            string(document.as_dictionary().unwrap(), "LSHandlerRank"),
            "Alternate"
        );
    }
    assert_eq!(project.url_schemes(), &[] as &[String]);

    let missing = Project::load(&path, options(Some("nope"), &[]));
    assert!(matches!(missing, Err(Error::Config(_))), "{missing:?}");
}

#[test]
fn a_missing_program_is_named() {
    let mut files = helper_files();
    files.retain(|(path, _)| !path.contains("ffmpeg"));
    let dir = staged("helper-app", &files);
    let project = Project::load(&dir.path().join("roadcase.toml"), options(None, &[])).unwrap();
    let error = macos::assemble(&project, &dir.path().join("out")).unwrap_err();
    assert!(
        matches!(&error, Error::Missing(text) if text.contains("example-ffmpeg-aarch64-apple-darwin")),
        "{error}"
    );
}

fn extension_files() -> Vec<(&'static str, String)> {
    vec![
        ("Cargo.toml", CARGO.into()),
        ("target/release/example-drive", "main".into()),
        ("target/swift/example-domains", "domains".into()),
        ("target/swift/DriveFileProvider", "provider".into()),
        ("macos/App/Drive.icns", "icns".into()),
        (
            "macos/App/App.entitlements",
            plist_file(
                "<key>com.apple.security.application-groups</key><array><string>{app_group}</string></array>",
            ),
        ),
        (
            "macos/FileProvider/FileProvider.entitlements",
            plist_file(
                "<key>com.apple.security.app-sandbox</key><true/>\
                 <key>com.apple.security.application-groups</key><array><string>{app_group}</string></array>",
            ),
        ),
        (
            "macos/FileProvider/Info.plist",
            plist_file(
                "<key>CFBundleDisplayName</key><string>{name}</string>\
                 <key>NSExtension</key><dict>\
                 <key>NSExtensionPointIdentifier</key><string>com.apple.fileprovider-nonui</string>\
                 <key>NSExtensionPrincipalClass</key><string>DriveFileProvider.Extension</string>\
                 </dict>",
            ),
        ),
    ]
}

#[test]
fn the_extension_app_assembles() {
    let dir = staged("extension-app", &extension_files());
    let project = Project::load(
        &dir.path().join("roadcase.toml"),
        options(None, &[("APPLE_TEAM_ID", "ABCDE12345")]),
    )
    .unwrap();
    let out = dir.path().join("out");
    let app = macos::assemble(&project, &out).unwrap();
    let contents = app.path.join("Contents");
    assert_eq!(app.path, out.join("Example Drive.app"));
    assert!(contents.join("MacOS/example-drive").is_file());
    assert!(contents.join("MacOS/example-domains").is_file());

    let plist = read(&contents.join("Info.plist"));
    assert_eq!(string(&plist, "CFBundleExecutable"), "example-drive");
    assert_eq!(string(&plist, "CFBundleIconFile"), "Example Drive");
    assert_eq!(string(&plist, "CFBundleShortVersionString"), "2.3.0");

    let appex = contents.join("PlugIns/DriveFileProvider.appex/Contents");
    assert_eq!(
        std::fs::read_to_string(appex.join("MacOS/DriveFileProvider")).unwrap(),
        "provider"
    );
    let extension = read(&appex.join("Info.plist"));
    assert_eq!(
        string(&extension, "CFBundleIdentifier"),
        "com.example.drive.DriveFileProvider"
    );
    assert_eq!(
        string(&extension, "CFBundleExecutable"),
        "DriveFileProvider"
    );
    assert_eq!(string(&extension, "CFBundlePackageType"), "XPC!");
    assert_eq!(string(&extension, "CFBundleDisplayName"), "Example Drive");
    assert_eq!(string(&extension, "CFBundleVersion"), "2.3.0-beta.1");
    assert_eq!(
        string(&extension, "AppGroup"),
        "ABCDE12345.com.example.drive"
    );

    let agent = read(&contents.join("Library/LaunchAgents/com.example.drive.agent.plist"));
    assert_eq!(string(&agent, "Label"), "com.example.drive.agent");
    assert_eq!(
        string(&agent, "BundleProgram"),
        "Contents/MacOS/example-drive"
    );
    assert_eq!(agent["RunAtLoad"].as_boolean(), Some(true));
    assert_eq!(
        agent["ProgramArguments"].as_array().unwrap()[1].as_string(),
        Some("--background")
    );
    assert_eq!(
        agent["AssociatedBundleIdentifiers"].as_array().unwrap()[0].as_string(),
        Some("com.example.drive")
    );

    // The tool, the extension, then the bundle; the tool signs as the app.
    let order: Vec<_> = app
        .signing
        .iter()
        .map(|step| {
            step.path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        order,
        [
            "example-domains",
            "DriveFileProvider.appex",
            "Example Drive.app"
        ]
    );
    assert_eq!(
        app.signing[0].identifier.as_deref(),
        Some("com.example.drive")
    );

    // Entitlements get the team's application group.
    let rendered =
        macos::render_entitlements(&project, &app.signing, &dir.path().join("entitlements"))
            .unwrap();
    let provider = read(rendered[1].entitlements.as_ref().unwrap());
    assert_eq!(
        provider["com.apple.security.application-groups"]
            .as_array()
            .unwrap()[0]
            .as_string(),
        Some("ABCDE12345.com.example.drive")
    );
}

#[test]
fn without_a_team_what_needs_one_says_so() {
    let dir = staged("extension-app", &extension_files());
    let project = Project::load(&dir.path().join("roadcase.toml"), options(None, &[])).unwrap();
    let error = macos::assemble(&project, &dir.path().join("out")).unwrap_err();
    assert!(
        matches!(&error, Error::Value(text) if text.contains("team")),
        "{error}"
    );
}

#[test]
fn an_extension_without_ns_extension_is_refused() {
    let mut files = extension_files();
    for (path, content) in &mut files {
        if *path == "macos/FileProvider/Info.plist" {
            *content = plist_file("<key>CFBundleName</key><string>x</string>");
        }
    }
    let dir = staged("extension-app", &files);
    let project = Project::load(
        &dir.path().join("roadcase.toml"),
        options(None, &[("APPLE_TEAM_ID", "ABCDE12345")]),
    )
    .unwrap();
    let error = macos::assemble(&project, &dir.path().join("out")).unwrap_err();
    assert!(
        matches!(&error, Error::Plist(text) if text.contains("NSExtension")),
        "{error}"
    );
}

#[test]
fn entitlements_are_rendered_for_each_signing_step() {
    let dir = staged("extension-app", &extension_files());
    let project = Project::load(
        &dir.path().join("roadcase.toml"),
        options(None, &[("APPLE_TEAM_ID", "ABCDE12345")]),
    )
    .unwrap();
    let out = dir.path().join("out");
    let app = macos::assemble(&project, &out).unwrap();
    let identity = macos::Identity {
        name: "Developer ID Application: Example Inc. (ABCDE12345)".into(),
        keychain: None,
        developer_id: true,
        team: Some("ABCDE12345".into()),
    };
    let flags = macos::SignFlags::of(&project, &identity).unwrap();
    let steps =
        macos::render_entitlements(&project, &app.signing, &out.join("entitlements")).unwrap();
    let commands: Vec<_> = steps
        .iter()
        .map(|step| macos::codesign_command(&identity, flags, step))
        .collect();

    // Inside out: the tool, the extension, then the bundle.
    let last = |index: usize| PathBuf::from(commands[index].arguments.last().unwrap());
    assert!(last(0).ends_with("Contents/MacOS/example-domains"));
    assert!(last(1).ends_with("Contents/PlugIns/DriveFileProvider.appex"));
    assert!(last(2).ends_with("Example Drive.app"));
    for command in &commands {
        assert_eq!(
            command.arguments[..6],
            [
                "--force",
                "--options",
                "runtime",
                "--timestamp",
                "--sign",
                "Developer ID Application: Example Inc. (ABCDE12345)"
            ]
        );
        assert!(
            !command
                .arguments
                .iter()
                .any(|argument| argument == "--deep")
        );
    }
    // The tool runs as the application: its identifier.
    let identifier = commands[0]
        .arguments
        .iter()
        .position(|argument| argument == "--identifier")
        .unwrap();
    assert_eq!(commands[0].arguments[identifier + 1], "com.example.drive");
    assert!(!commands[2].arguments.iter().any(|a| a == "--identifier"));
    // Each step has its own rendered entitlements, the team filled in.
    for (index, step) in steps.iter().enumerate() {
        let entitlements = step.entitlements.as_ref().unwrap();
        let position = commands[index]
            .arguments
            .iter()
            .position(|argument| argument == "--entitlements")
            .unwrap();
        assert_eq!(
            commands[index].arguments[position + 1],
            entitlements.display().to_string()
        );
        let text = std::fs::read_to_string(entitlements).unwrap();
        assert!(!text.contains("{team}"), "{text}");
    }
    let extension = std::fs::read_to_string(steps[1].entitlements.as_ref().unwrap()).unwrap();
    assert!(
        extension.contains("ABCDE12345.com.example.drive"),
        "{extension}"
    );
}

#[test]
fn a_release_is_made_on_its_own_system() {
    if cfg!(target_os = "macos") {
        return;
    }
    let dir = staged("helper-app", &helper_files());
    let project = Project::load(&dir.path().join("roadcase.toml"), options(None, &[])).unwrap();
    let error = macos::release(
        &project,
        &macos::Identity::ad_hoc(),
        &dir.path().join("out"),
    )
    .unwrap_err();
    assert!(matches!(&error, Error::Config(text) if text.contains("on a Mac")));
}
