//! The Windows installer of the helper-app example: what is staged, the
//! defines and the generated include `makensis` is given, and the signing
//! command. No Windows and no signing tool is needed; with `makensis` on
//! `PATH`, the example script is also compiled into an installer.

use roadcase_bundle::config::Signed;
use roadcase_bundle::{Environment, Host, Options, Project, process, windows};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn example(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/helper-app")
        .join(path)
}

fn staged() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::copy(example("roadcase.toml"), root.join("roadcase.toml")).unwrap();
    for (path, content) in [
        (
            "Cargo.toml",
            "[package]\nname = \"x\"\nversion = \"2.3.0-beta.1\"\n",
        ),
        (
            "target/x86_64-pc-windows-msvc/release/example-studio.exe",
            "main",
        ),
        (
            "binaries/example-plugin-host-x86_64-pc-windows-msvc.exe",
            "host",
        ),
        (
            "binaries/example-ffmpeg-x86_64-pc-windows-msvc.exe",
            "ffmpeg",
        ),
        ("docs/guide.md", "# Guide"),
        ("docs/more/cues.md", "# Cues"),
        ("icons/icon.ico", "ico"),
    ] {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    let script = root.join("packaging/windows/installer.nsi");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::copy(example("packaging/windows/installer.nsi"), script).unwrap();
    dir
}

fn project(dir: &Path, variant: Option<&str>, env: &[(&str, &str)]) -> Project {
    let options = Options {
        variant: variant.map(str::to_owned),
        host: Host::windows(),
        environment: Environment::Fixed(
            env.iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect::<BTreeMap<_, _>>(),
        ),
    };
    Project::load(&dir.join("roadcase.toml"), options).unwrap()
}

fn value<'a>(defines: &'a [(String, String)], name: &str) -> Option<&'a str> {
    defines
        .iter()
        .find(|(found, _)| found == name)
        .map(|(_, value)| value.as_str())
}

#[test]
fn the_install_folder_is_staged() {
    let dir = staged();
    let project = project(dir.path(), None, &[]);
    let stage = windows::stage(&project, &dir.path().join("out")).unwrap();

    assert!(stage.folder.is_absolute());
    assert!(stage.folder.ends_with("out/Example Studio.windows"));
    assert_eq!(stage.executable, stage.folder.join("Example Studio.exe"));
    assert_eq!(
        stage.files,
        [
            "Example Studio.exe",
            "example-ffmpeg.exe",
            "example-plugin-host.exe",
            "resources/docs/guide.md",
            "resources/docs/more/cues.md",
        ]
        .map(PathBuf::from)
    );
    assert_eq!(
        std::fs::read_to_string(stage.folder.join("example-plugin-host.exe")).unwrap(),
        "host"
    );
    assert_eq!(stage.resources.len(), 1);
    assert_eq!(stage.resources[0].1, stage.folder.join("resources"));
}

#[test]
fn makensis_is_given_the_config_as_defines() {
    let dir = staged();
    let project = project(dir.path(), None, &[]);
    let out = dir.path().join("out");
    let stage = windows::stage(&project, &out).unwrap();
    let installer = out.join("Example Studio-Setup.exe");
    let include = out.join("Example Studio.windows.nsh");
    let defines = windows::defines(&project, &stage, &installer, &include, None).unwrap();
    let value = |name: &str| value(&defines, name);

    assert_eq!(value("NAME"), Some("Example Studio"));
    assert_eq!(value("ID"), Some("com.example.studio"));
    assert_eq!(value("VERSION"), Some("2.3.0-beta.1"));
    assert_eq!(value("VERSION4"), Some("2.3.0.0"));
    assert_eq!(value("PUBLISHER"), Some("Example Inc."));
    assert_eq!(value("COPYRIGHT"), Some("Copyright Example Inc."));
    assert_eq!(value("EXE_NAME"), Some("Example Studio.exe"));
    assert_eq!(value("EXE"), stage.executable.to_str());
    assert_eq!(value("STAGE"), stage.folder.to_str());
    assert_eq!(value("OUT"), installer.to_str());
    assert_eq!(value("ROADCASE_NSH"), include.to_str());
    assert_eq!(
        value("PROGRAM_EXAMPLE_PLUGIN_HOST"),
        stage.folder.join("example-plugin-host.exe").to_str()
    );
    assert_eq!(
        value("PROGRAM_EXAMPLE_FFMPEG"),
        stage.folder.join("example-ffmpeg.exe").to_str()
    );
    assert_eq!(
        value("RESOURCE_RESOURCES"),
        stage.folder.join("resources").to_str()
    );
    assert!(value("ICON").unwrap().ends_with("icon.ico"));
    assert_eq!(value("INSTALL_MODE"), Some("current-user"));
    assert_eq!(
        value("UNINSTALL_KEY"),
        Some("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Example Studio")
    );
    assert_eq!(
        value("OPEN_COMMAND"),
        Some("\"$INSTDIR\\Example Studio.exe\" --open \"%1\"")
    );
    assert_eq!(value("URL_COMMAND"), value("OPEN_COMMAND"));
    assert_eq!(value("URL_SCHEMES"), Some("examplestudio"));
    assert_eq!(value("SILENT_FLAG"), Some("/S"));
    assert_eq!(value("UPDATE_FLAG"), Some("/UPDATE"));
    assert_eq!(value("RELAUNCH_FLAG"), Some("/RELAUNCH="));
    assert_eq!(value("ALSO_ACCEPT_1"), Some("/R"));
    // Not signing: the script's !ifdef leaves the uninstaller alone.
    assert_eq!(value("SIGN_UNINSTALLER"), None);

    let script = dir.path().join("packaging/windows/installer.nsi");
    let command = windows::makensis_command("makensis", &defines, &script, None);
    assert_eq!(command.program, "makensis");
    assert_eq!(command.arguments[0], "-V2");
    assert!(
        command
            .arguments
            .contains(&"-DVERSION=2.3.0-beta.1".to_owned())
    );
    assert_eq!(
        command.arguments.last().map(String::as_str),
        script.to_str()
    );
    assert_eq!(command.arguments.len(), defines.len() + 2);
}

#[test]
fn the_include_installs_registers_and_reads_the_update_flags() {
    let dir = staged();
    let project = project(dir.path(), None, &[]);
    let stage = windows::stage(&project, &dir.path().join("out")).unwrap();
    let include = windows::include(&project, &stage).unwrap();
    // As the system writes the path: makensis reads its files from there.
    let source = |file: &str| {
        file.split('/')
            .fold(stage.folder.clone(), |path, part| path.join(part))
            .display()
            .to_string()
    };

    // Programs may be running: they are replaced by renaming. Resources are
    // written into their folders.
    assert!(include.contains(&format!(
        "  !insertmacro ROADCASE_REPLACE \"Example Studio.exe\" \"{}\"\n",
        source("Example Studio.exe")
    )));
    assert!(include.contains(&format!(
        "  SetOutPath \"$INSTDIR\\resources\\docs\"\n  File \"{}\"\n  SetOutPath \"$INSTDIR\\resources\\docs\\more\"\n  File \"{}\"\n  SetOutPath \"$INSTDIR\"\n!macroend",
        source("resources/docs/guide.md"),
        source("resources/docs/more/cues.md")
    )));
    // Uninstalling removes each file, then the folders, deepest first.
    let removed = include
        .split("!macro ROADCASE_UNINSTALL_FILES\n")
        .nth(1)
        .unwrap()
        .split("!macroend")
        .next()
        .unwrap();
    assert_eq!(
        removed,
        "  Delete \"$INSTDIR\\Example Studio.exe\"\n  Delete \"$INSTDIR\\example-ffmpeg.exe\"\n  Delete \"$INSTDIR\\example-plugin-host.exe\"\n  Delete \"$INSTDIR\\resources\\docs\\guide.md\"\n  Delete \"$INSTDIR\\resources\\docs\\more\\cues.md\"\n  Delete /REBOOTOK \"$INSTDIR\\*.old\"\n  RMDir \"$INSTDIR\\resources\\docs\\more\"\n  RMDir \"$INSTDIR\\resources\\docs\"\n  RMDir \"$INSTDIR\\resources\"\n"
    );

    // The URL scheme, with the command the config gives.
    let command = "\"$\\\"$INSTDIR\\Example Studio.exe$\\\" --open $\\\"%1$\\\"\"";
    assert!(include.contains(
        "  WriteRegStr HKCU \"Software\\Classes\\examplestudio\" \"\" \"URL:Example Studio\"\n  WriteRegStr HKCU \"Software\\Classes\\examplestudio\" \"URL Protocol\" \"\"\n"
    ));
    assert!(include.contains(&format!(
        "  WriteRegStr HKCU \"Software\\Classes\\examplestudio\\shell\\open\\command\" \"\" {command}\n"
    )));
    // A document type the application owns becomes the default, the earlier
    // one kept for the uninstaller.
    assert!(include.contains(
        "  WriteRegStr HKCU \"Software\\Classes\\.exproj\" \"com.example.studio.ExampleProject_backup\" \"$R0\"\n"
    ));
    assert!(include.contains(
        "  WriteRegStr HKCU \"Software\\Classes\\.exproj\" \"\" \"com.example.studio.ExampleProject\"\n"
    ));
    assert!(include.contains(&format!(
        "  WriteRegStr HKCU \"Software\\Classes\\com.example.studio.ExampleProject\\shell\\open\\command\" \"\" {command}\n"
    )));
    assert!(
        include.contains(
            "  DeleteRegKey HKCU \"Software\\Classes\\com.example.studio.ExampleVenue\"\n"
        )
    );
    // The updater's flags, and the one an earlier updater passed.
    for flag in ["\"/UPDATE\" $R3", "\"/R\" $R3", "\"/RELAUNCH=\" $R1"] {
        assert!(
            include.contains(&format!("${{GetOptions}} $R2 {flag}")),
            "{flag}"
        );
    }
}

#[test]
fn the_test_variant_registers_no_scheme_and_claims_no_default() {
    let dir = staged();
    let project = project(dir.path(), Some("test"), &[]);
    let associations = windows::associations(&project).unwrap();
    assert_eq!(associations.len(), 2);
    assert!(associations.iter().all(|association| !association.default));
    assert_eq!(
        associations[0].prog_id,
        "com.example.studio.test.ExampleProject"
    );
    let stage = windows::stage(&project, &dir.path().join("out")).unwrap();
    let include = windows::include(&project, &stage).unwrap();
    assert!(!include.contains("URL Protocol"));
    assert!(!include.contains("_backup\" \"$R0\""));
    assert!(
        include.contains("\\.exproj\\OpenWithProgids\" \"com.example.studio.test.ExampleProject\"")
    );
}

#[test]
fn signing_runs_on_windows_with_its_credentials_only() {
    let dir = staged();
    let azure = [
        ("AZURE_CLIENT_SECRET", "secret"),
        ("AZURE_ACCOUNT_NAME", "account"),
        ("AZURE_CERT_PROFILE_NAME", "profile"),
    ];
    // Without the variable when_env names, or off Windows: unsigned.
    assert_eq!(
        windows::signer(&project(dir.path(), None, &[]), true).unwrap(),
        None
    );
    let with_credentials = project(dir.path(), None, &azure);
    assert_eq!(windows::signer(&with_credentials, false).unwrap(), None);

    let signer = windows::signer(&with_credentials, true).unwrap().unwrap();
    for target in [
        Signed::Executable,
        Signed::Programs,
        Signed::Uninstaller,
        Signed::Installer,
    ] {
        assert!(signer.signs(target));
    }
    let command = signer.invocation(Path::new("C:\\out\\Example Studio.exe"));
    assert_eq!(command.program, "artifact-signing-cli");
    assert_eq!(
        command.arguments,
        [
            "-e",
            "https://eus.codesigning.azure.net/",
            "-d",
            "Example Studio",
            "C:\\out\\Example Studio.exe",
        ]
    );
    assert_eq!(
        command.env,
        [
            ("AZURE_ARTIFACT_SIGNING_ACCOUNT".into(), "account".into()),
            (
                "AZURE_ARTIFACT_SIGNING_CERTIFICATE_PROFILE".into(),
                "profile".into()
            ),
        ]
    );
    // What makensis runs on the uninstaller while it writes the installer.
    assert_eq!(
        signer.uninstaller_command(),
        "artifact-signing-cli -e https://eus.codesigning.azure.net/ -d \"Example Studio\" \"%1\""
    );
    let out = dir.path().join("out");
    let stage = windows::stage(&with_credentials, &out).unwrap();
    let defines = windows::defines(
        &with_credentials,
        &stage,
        &out.join("setup.exe"),
        &out.join("include.nsh"),
        Some(&signer),
    )
    .unwrap();
    assert_eq!(
        value(&defines, "SIGN_UNINSTALLER"),
        Some(signer.uninstaller_command().as_str())
    );
    let makensis = windows::makensis_command(
        "makensis",
        &defines,
        Path::new("installer.nsi"),
        Some(&signer),
    );
    assert_eq!(makensis.env, signer.env);
}

#[test]
fn a_missing_script_is_named() {
    let dir = staged();
    std::fs::remove_file(dir.path().join("packaging/windows/installer.nsi")).unwrap();
    let project = project(dir.path(), None, &[]);
    let error = windows::installer(&project, &dir.path().join("out")).unwrap_err();
    assert!(error.to_string().contains("installer.nsi"), "{error}");
}

/// The smallest icon makensis reads: one pixel, 32 bits.
fn icon() -> Vec<u8> {
    let mut icon = vec![0, 0, 1, 0, 1, 0]; // reserved, type 1 (icon), one image
    icon.extend([1, 1, 0, 0, 1, 0, 32, 0]); // 1x1, no palette, 1 plane, 32 bits
    icon.extend(48u32.to_le_bytes()); // the image's size
    icon.extend(22u32.to_le_bytes()); // and where it starts
    icon.extend(40u32.to_le_bytes()); // BITMAPINFOHEADER
    icon.extend(1i32.to_le_bytes()); // width
    icon.extend(2i32.to_le_bytes()); // height: the colour and the mask rows
    icon.extend([1, 0, 32, 0]); // 1 plane, 32 bits
    icon.extend([0; 24]); // no compression, sizes and palette left to the reader
    icon.extend([0xff, 0x80, 0x00, 0xff]); // one pixel
    icon.extend([0; 4]); // its mask row
    icon
}

/// With NSIS installed, the example script compiles into an installer from
/// the defines and the include, on any system. Without it, there is nothing
/// to run.
#[test]
fn the_example_script_compiles_where_makensis_is_installed() {
    if !process::on_path("makensis") {
        eprintln!("makensis is not on PATH: not compiled");
        return;
    }
    let dir = staged();
    std::fs::write(dir.path().join("icons/icon.ico"), icon()).unwrap();
    // The process environment, for PATH; no signing credentials in it matter
    // off Windows, and on Windows the example's when_env is not set.
    let options = Options {
        variant: None,
        host: Host::windows(),
        environment: Environment::Fixed(BTreeMap::new()),
    };
    let project = Project::load(&dir.path().join("roadcase.toml"), options).unwrap();
    let installer = windows::installer(&project, &dir.path().join("out")).unwrap();
    assert!(!installer.signed);
    assert!(installer.path.ends_with("out/Example Studio-Setup.exe"));
    let bytes = std::fs::read(&installer.path).unwrap();
    assert_eq!(&bytes[..2], b"MZ");
    assert!(bytes.len() > 30_000, "{}", bytes.len());
}
