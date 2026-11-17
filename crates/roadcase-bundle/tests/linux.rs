//! The helper app's `[linux]` assembles into the AppDir it describes, from
//! fixture files in a temporary folder, on any system. Splitting symbols and
//! making the AppImage need Linux's tools; the test for the first runs on
//! Linux only.

use roadcase_bundle::{Environment, Error, Host, Options, Project, linux};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/helper-app/roadcase.toml")
}

fn options(variant: Option<&str>) -> Options {
    Options {
        variant: variant.map(str::to_owned),
        host: Host {
            arch: "x86_64".into(),
            triple: "x86_64-unknown-linux-gnu".into(),
        },
        environment: Environment::Fixed(BTreeMap::new()),
    }
}

fn png(size: u32) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    bytes.extend_from_slice(&size.to_be_bytes());
    bytes.extend_from_slice(&size.to_be_bytes());
    bytes.extend_from_slice(b"\x08\x06\0\0\0");
    bytes
}

fn staged(skip: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(example(), dir.path().join("roadcase.toml")).unwrap();
    let files: Vec<(&str, Vec<u8>)> = vec![
        ("Cargo.toml", b"[package]\nversion = \"2.3.0-beta.1\"\n".to_vec()),
        ("target/release/example-studio", b"main".to_vec()),
        (
            "binaries/example-plugin-host-x86_64-unknown-linux-gnu",
            b"host".to_vec(),
        ),
        (
            "binaries/example-ffmpeg-x86_64-unknown-linux-gnu",
            b"ffmpeg".to_vec(),
        ),
        ("docs/guide.md", b"# Guide".to_vec()),
        ("docs/notes.txt", b"not a manual page".to_vec()),
        ("packaging/linux/repository.gpg", b"\x99\x00binary key".to_vec()),
        ("icons/linux/256.png", png(256)),
        ("icons/linux/512.png", png(512)),
        ("icons/linux/icon.svg", b"<svg/>".to_vec()),
        (
            "packaging/linux/studio.metainfo.xml",
            b"<component><id>{id}</id><releases><release version=\"{version}\"/></releases></component>"
                .to_vec(),
        ),
    ];
    for (path, content) in files {
        if path.contains(skip) && !skip.is_empty() {
            continue;
        }
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    dir
}

#[test]
fn the_helper_app_assembles_for_linux() {
    let dir = staged("");
    let project = Project::load(&dir.path().join("roadcase.toml"), options(None)).unwrap();
    let out = dir.path().join("out");
    let app = linux::assemble(&project, &out).unwrap();

    assert_eq!(app.path, out.join("Example Studio.AppDir"));
    let root = &app.path;
    let read = |path: &str| std::fs::read_to_string(root.join(path)).unwrap();
    assert_eq!(read("usr/bin/example-studio"), "main");
    assert_eq!(read("usr/bin/example-plugin-host"), "host");
    assert_eq!(read("usr/bin/example-ffmpeg"), "ffmpeg");
    assert_eq!(read("usr/share/example-studio/docs/guide.md"), "# Guide");
    assert!(
        !root
            .join("usr/share/example-studio/docs/notes.txt")
            .exists()
    );
    assert_eq!(
        app.symbols,
        [
            root.join("usr/bin/example-plugin-host"),
            root.join("usr/bin/example-studio")
        ]
    );

    // Icons in the hicolor theme; the largest PNG on top, and .DirIcon.
    let icons = root.join("usr/share/icons/hicolor");
    assert!(icons.join("256x256/apps/com.example.studio.png").is_file());
    assert!(icons.join("512x512/apps/com.example.studio.png").is_file());
    assert_eq!(
        read("usr/share/icons/hicolor/scalable/apps/com.example.studio.svg"),
        "<svg/>"
    );
    assert_eq!(
        std::fs::read(root.join("com.example.studio.png")).unwrap(),
        png(512)
    );
    #[cfg(unix)]
    assert_eq!(
        std::fs::read_link(root.join(".DirIcon")).unwrap(),
        Path::new("com.example.studio.png")
    );

    // The desktop entry, at the top for appimagetool and in applications.
    let entry = read("com.example.studio.desktop");
    assert_eq!(
        entry,
        read("usr/share/applications/com.example.studio.desktop")
    );
    for line in [
        "[Desktop Entry]",
        "Type=Application",
        "Name=Example Studio",
        "Comment=Plan and run shows with live video and audio",
        "Exec=example-studio --open %u",
        "Icon=com.example.studio",
        "Terminal=false",
        "Categories=AudioVideo;Video;",
        "MimeType=x-scheme-handler/examplestudio;application/x-example-studio-project;application/x-exvenue;",
        "X-AppImage-Version=2.3.0-beta.1",
        "StartupWMClass=example-studio",
        "Keywords=show;cue;video;",
    ] {
        assert!(
            entry.lines().any(|found| found == line),
            "no {line} in\n{entry}"
        );
    }

    // The two owned types, declared with their globs.
    let mime = read("usr/share/mime/packages/com.example.studio.xml");
    assert!(mime.contains("<mime-type type=\"application/x-example-studio-project\">"));
    assert!(mime.contains("<comment>Example Project</comment>"));
    assert!(mime.contains("<glob pattern=\"*.exproj\"/>"));
    assert!(mime.contains("<mime-type type=\"application/x-exvenue\">"));
    assert!(mime.contains("<glob pattern=\"*.exvenue\"/>"));

    assert_eq!(
        read("usr/share/metainfo/com.example.studio.metainfo.xml"),
        "<component><id>com.example.studio</id><releases><release version=\"2.3.0-beta.1\"/></releases></component>"
    );

    let run = read("AppRun");
    assert!(run.contains("exec \"$HERE/usr/bin/\"'example-studio' \"$@\""));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in ["AppRun", "usr/bin/example-ffmpeg", "usr/bin/example-studio"] {
            let mode = std::fs::metadata(root.join(path))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755, "{path}");
        }
    }

    // Assembling again replaces the AppDir.
    std::fs::write(root.join("stale"), "x").unwrap();
    linux::assemble(&project, &out).unwrap();
    assert!(!root.join("stale").exists());
}

#[test]
fn the_test_variant_declares_and_claims_nothing() {
    let dir = staged("");
    let project = Project::load(&dir.path().join("roadcase.toml"), options(Some("test"))).unwrap();
    let app = linux::assemble(&project, &dir.path().join("out")).unwrap();
    let entry = std::fs::read_to_string(app.path.join("com.example.studio.test.desktop")).unwrap();
    assert!(entry.contains("Icon=com.example.studio.test\n"));
    // No URL scheme; the documents, which the real application declares.
    assert!(
        entry.contains("MimeType=application/x-example-studio-project;application/x-exvenue;\n")
    );
    assert!(!app.path.join("usr/share/mime").exists());
    assert!(
        app.path
            .join("usr/share/icons/hicolor/256x256/apps/com.example.studio.test.png")
            .is_file()
    );
}

#[test]
fn the_defaults_follow_what_the_application_opens() {
    let config = |extra: &str| {
        format!(
            r#"
[app]
name = "Tiny Tool"
id = "com.example.tiny"
version = "0.1.0"
{extra}

[linux]
icons = ["icon.svg"]
categories = ["Utility"]

[linux.executable]
from = "tiny"
"#
        )
    };
    let entry = |extra: &str| {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tiny"), "tiny").unwrap();
        std::fs::write(dir.path().join("icon.svg"), "<svg/>").unwrap();
        let project = Project::parse(&config(extra), dir.path(), options(None)).unwrap();
        let app = linux::assemble(&project, &dir.path().join("out")).unwrap();
        assert!(app.path.join("usr/bin/tiny-tool").is_file());
        assert!(app.path.join("com.example.tiny.svg").is_file());
        std::fs::read_to_string(app.path.join("com.example.tiny.desktop")).unwrap()
    };
    let plain = entry("");
    assert!(plain.contains("\nExec=tiny-tool\n"), "{plain}");
    assert!(!plain.contains("MimeType"));

    let documents = entry(
        "[[document_type]]\nname = \"Note\"\nextensions = [\"note\"]\n\
         [[document_type]]\nname = \"Picture\"\nextensions = [\"png\"]\nmime_type = \"image/png\"\nrank = \"alternate\"\nrole = \"viewer\"",
    );
    assert!(documents.contains("\nExec=tiny-tool %F\n"), "{documents}");
    assert!(documents.contains("\nMimeType=application/x-note;image/png;\n"));

    let links = entry("url_schemes = [\"tiny\"]");
    assert!(links.contains("\nExec=tiny-tool %u\n"));
    assert!(links.contains("\nMimeType=x-scheme-handler/tiny;\n"));
}

#[test]
fn a_missing_program_or_icon_is_named() {
    let dir = staged("ffmpeg");
    let project = Project::load(&dir.path().join("roadcase.toml"), options(None)).unwrap();
    let error = linux::assemble(&project, &dir.path().join("out")).unwrap_err();
    assert!(
        matches!(&error, Error::Missing(text) if text.contains("example-ffmpeg-x86_64-unknown-linux-gnu")),
        "{error}"
    );

    let dir = staged("512.png");
    let project = Project::load(&dir.path().join("roadcase.toml"), options(None)).unwrap();
    let error = linux::assemble(&project, &dir.path().join("out")).unwrap_err();
    assert!(
        matches!(&error, Error::Missing(text) if text.contains("512.png")),
        "{error}"
    );
}

#[test]
fn an_icon_that_is_not_square_is_refused() {
    let dir = staged("");
    let mut wide = png(512);
    wide[20..24].copy_from_slice(&256u32.to_be_bytes());
    std::fs::write(dir.path().join("icons/linux/512.png"), wide).unwrap();
    let project = Project::load(&dir.path().join("roadcase.toml"), options(None)).unwrap();
    let error = linux::assemble(&project, &dir.path().join("out")).unwrap_err();
    assert!(
        matches!(&error, Error::Config(text) if text.contains("square")),
        "{error}"
    );
}

#[test]
fn an_appimage_needs_a_runtime_on_this_machine() {
    let dir = staged("");
    let project = Project::load(&dir.path().join("roadcase.toml"), options(None)).unwrap();
    let out = dir.path().join("out");
    let app = linux::assemble(&project, &out).unwrap();
    let error = linux::appimage(&project, &app, &out).unwrap_err();
    assert!(
        matches!(&error, Error::Missing(text) if text.contains("APPIMAGE_RUNTIME")),
        "{error}"
    );
}

#[test]
fn what_linux_cannot_use_is_reported() {
    let toml = r#"
[app]
name = "Example"
id = "com.example.app"
version = "1.0.0"

[[document_type]]
name = "Thing"
extensions = []
content_types = ["com.example.thing"]

[[document_type]]
name = "Other"
extensions = ["other"]
mime_type = "not a type"

[linux]
icons = ["icon.ico"]
exec_arguments = ["%u", "%F"]
desktop = { Exec = "mine", "bad key" = "x" }

[linux.executable]
from = "example"
name = "example"

[[linux.program]]
name = "example"
from = "other"
"#;
    let config: roadcase_bundle::Config = toml::from_str(toml).unwrap();
    let found = roadcase_bundle::problems(&config);
    let expect = [
        "document_type[0] has no extensions, so Linux needs its mime_type",
        "document_type[1].mime_type",
        "linux.icons: icon.ico",
        "linux.categories is empty",
        "more than one of %f, %F, %u and %U",
        "linux.desktop.Exec is written",
        "linux.desktop.bad key",
        "example is in usr/bin twice",
    ];
    for needle in expect {
        assert!(
            found.iter().any(|problem| problem.contains(needle)),
            "no problem mentions {needle:?}:\n{}",
            found.join("\n")
        );
    }
    assert_eq!(found.len(), expect.len(), "{}", found.join("\n"));
}

// The tree holds a symbolic link in /usr/bin, which only Unix can make.
#[cfg(unix)]
#[test]
fn the_helper_app_becomes_a_package_tree() {
    let dir = staged("");
    let project = Project::load(&dir.path().join("roadcase.toml"), options(None)).unwrap();
    let out = dir.path().join("out");
    let app = linux::assemble(&project, &out).unwrap();
    assert_eq!(linux::deb_package(&project).unwrap(), "example-studio");
    assert_eq!(linux::pacman_package(&project).unwrap(), "example-studio");

    let root = out.join("tree");
    linux::package_tree(
        &project,
        &app,
        &root,
        "example-studio",
        roadcase_update::PackageManager::Apt,
    )
    .unwrap();
    let read = |path: &str| std::fs::read_to_string(root.join(path)).unwrap();
    let prefix = "opt/example-studio";
    assert_eq!(read(&format!("{prefix}/bin/example-studio")), "main");
    assert_eq!(read(&format!("{prefix}/bin/example-plugin-host")), "host");
    assert_eq!(
        read(&format!("{prefix}/share/example-studio/docs/guide.md")),
        "# Guide"
    );
    assert_eq!(read(&format!("{prefix}/.roadcase-install")), "deb\n");
    assert!(
        root.join("usr/share/icons/hicolor/512x512/apps/com.example.studio.png")
            .is_file()
    );
    assert!(
        root.join("usr/share/mime/packages/com.example.studio.xml")
            .is_file()
    );
    assert!(
        root.join("usr/share/metainfo/com.example.studio.metainfo.xml")
            .is_file()
    );
    // Nothing of the AppImage's own.
    for absent in [
        "AppRun",
        "com.example.studio.desktop",
        ".DirIcon",
        "opt/example-studio/share/applications",
        "opt/example-studio/share/icons",
    ] {
        assert!(!root.join(absent).exists(), "{absent}");
    }
    let entry = read("usr/share/applications/com.example.studio.desktop");
    assert!(
        entry.contains("\nExec=example-studio --open %u\n"),
        "{entry}"
    );
    assert!(entry.contains("\nIcon=com.example.studio\n"));
    assert!(!entry.contains("X-AppImage"), "{entry}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::read_link(root.join("usr/bin/example-studio")).unwrap(),
            Path::new("/opt/example-studio/bin/example-studio")
        );
        let mode = |path: &str| {
            std::fs::symlink_metadata(root.join(path))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode(&format!("{prefix}/bin/example-studio")), 0o755);
        assert_eq!(
            mode(&format!("{prefix}/share/example-studio/docs/guide.md")),
            0o644
        );
        assert_eq!(mode("usr/share/applications"), 0o755);
    }

    // The same tree for pacman says so.
    linux::package_tree(
        &project,
        &app,
        &root,
        "example-studio",
        roadcase_update::PackageManager::Pacman,
    )
    .unwrap();
    assert_eq!(read(&format!("{prefix}/.roadcase-install")), "pacman\n");
}

#[test]
fn the_helper_app_describes_its_packages() {
    let dir = staged("");
    let project = Project::load(&dir.path().join("roadcase.toml"), options(None)).unwrap();
    let depends = linux::merge_depends(
        &["libgtk-3-0".into()],
        &linux::parse_shlibdeps(
            "shlibs:Depends=libc6 (>= 2.34), libgcc-s1 (>= 4.2), libgtk-3-0 (>= 3.0)\n",
        ),
    );
    assert_eq!(
        depends,
        ["libgtk-3-0", "libc6 (>= 2.34)", "libgcc-s1 (>= 4.2)"]
    );
    assert_eq!(
        linux::deb_control(&project, &depends, 2048).unwrap(),
        "Package: example-studio\n\
         Version: 2.3.0~beta.1-1\n\
         Architecture: amd64\n\
         Maintainer: Example Studio <packages@example.com>\n\
         Installed-Size: 2048\n\
         Depends: libgtk-3-0, libc6 (>= 2.34), libgcc-s1 (>= 4.2)\n\
         Recommends: xdg-utils\n\
         Section: video\n\
         Priority: optional\n\
         Homepage: https://example.com/studio\n\
         Description: Plan and run shows with live video and audio\n \
         Plan cues and run them live, with video and audio from one window.\n \
         .\n \
         Includes the plugin host and an ffmpeg build.\n"
    );
    assert_eq!(
        linux::apt_source(
            &project,
            "/usr/share/keyrings/example-studio-archive-keyring.gpg"
        )
        .unwrap()
        .unwrap(),
        "Types: deb\n\
         URIs: https://packages.example.com/studio/apt\n\
         Suites: stable\n\
         Components: main\n\
         Signed-By: /usr/share/keyrings/example-studio-archive-keyring.gpg\n"
    );

    let pkgbuild = linux::pkgbuild(&project, Path::new("/work/it's here")).unwrap();
    for line in [
        "pkgname='example-studio'",
        "pkgver='2.3.0beta.1'",
        "pkgrel='1'",
        "pkgdesc='Plan and run shows with live video and audio'",
        "arch=('x86_64')",
        "url='https://example.com/studio'",
        "license=('MIT')",
        "depends=('gtk3')",
        "optdepends=('xdg-utils: open links')",
        "options=('!strip' '!debug')",
        "  cp -a '/work/it'\\''s here'/. \"$pkgdir\"/",
    ] {
        assert!(
            pkgbuild.lines().any(|found| found == line),
            "no {line} in\n{pkgbuild}"
        );
    }
}

#[test]
fn versions_become_package_versions() {
    assert_eq!(linux::deb_version("1.2.0", "1"), "1.2.0-1");
    assert_eq!(linux::deb_version("1.2.0-beta.1", "2"), "1.2.0~beta.1-2");
    assert_eq!(
        linux::deb_version("1.2.0-rc-2+abc", "1"),
        "1.2.0~rc-2+abc-1"
    );
    assert_eq!(linux::pacman_version("1.2.0"), "1.2.0");
    assert_eq!(linux::pacman_version("1.2.0-beta.1"), "1.2.0beta.1");
    assert_eq!(linux::pacman_version("1.2.0-rc-2+abc"), "1.2.0rc_2");
    assert_eq!(linux::deb_arch("aarch64").unwrap(), "arm64");
    assert_eq!(linux::pacman_arch("aarch64").unwrap(), "aarch64");
    assert!(linux::deb_arch("riscv64").is_err());
}

#[test]
fn what_packages_cannot_use_is_reported() {
    let toml = r#"
[app]
name = "Example App"
id = "com.example.app"
version = "1.0.0"

[linux]
icons = ["icon.svg"]
categories = ["Utility"]

[linux.executable]
from = "example"
name = "Example_App"

[linux.deb]
revision = "one"

[linux.deb.apt_source]
url = "ftp://example.com"
components = ["main/../x"]
key = "key.gpg"

[linux.pacman]
pkgname = "-bad"
pkgrel = "1."
"#;
    let config: roadcase_bundle::Config = toml::from_str(toml).unwrap();
    let found = roadcase_bundle::problems(&config);
    let expect = [
        "linux.deb.package \"Example_App\"",
        "linux.deb.revision",
        "linux.deb.maintainer (or app.publisher)",
        "linux.deb.apt_source.url",
        "linux.deb.apt_source.components",
        "linux.pacman.pkgname \"-bad\"",
        "linux.pacman.pkgrel",
    ];
    for needle in expect {
        assert!(
            found.iter().any(|problem| problem.contains(needle)),
            "no problem mentions {needle:?}:\n{}",
            found.join("\n")
        );
    }
    assert_eq!(found.len(), expect.len(), "{}", found.join("\n"));
}

/// Splits the symbols of a real executable (this test's own, which has
/// debug information), strips it and links the two.
#[cfg(target_os = "linux")]
#[test]
fn symbols_go_into_debug_files_and_a_zip() {
    let dir = staged("");
    let this = std::env::current_exe().unwrap();
    std::fs::copy(&this, dir.path().join("target/release/example-studio")).unwrap();
    let project = Project::load(&dir.path().join("roadcase.toml"), options(None)).unwrap();
    let out = dir.path().join("out");
    let mut app = linux::assemble(&project, &out).unwrap();
    app.symbols.retain(|path| path.ends_with("example-studio"));
    let before = std::fs::metadata(&app.symbols[0]).unwrap().len();

    let zip = linux::symbols(&project, &app, &out).unwrap();
    assert_eq!(
        zip,
        out.join("Example Studio-2.3.0-beta.1-x86_64.debug.zip")
    );
    assert!(zip.is_file());
    let debug = out.join("Example Studio.debug/example-studio.debug");
    assert!(std::fs::metadata(&debug).unwrap().len() > 0);
    let after = std::fs::metadata(&app.symbols[0]).unwrap().len();
    assert!(after < before, "{after} is not smaller than {before}");
    let sections = std::process::Command::new("objdump")
        .args(["-h"])
        .arg(&app.symbols[0])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&sections.stdout).contains(".gnu_debuglink"));
}
