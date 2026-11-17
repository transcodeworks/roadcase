//! What a config may not say.

use roadcase_bundle::{Config, Environment, Error, Host, Options, Project, problems};
use std::path::Path;

const MINIMAL: &str = r#"
[app]
name = "Example"
id = "com.example.app"
version = "1.0.0"

[macos.executable]
from = "target/release/example"
"#;

fn problems_of(toml: &str) -> Vec<String> {
    let config: Config = toml::from_str(toml).unwrap();
    problems(&config)
}

fn options() -> Options {
    Options {
        variant: None,
        host: Host::macos(),
        environment: Environment::Fixed(Default::default()),
    }
}

#[test]
fn a_minimal_config_is_fine() {
    assert_eq!(problems_of(MINIMAL), Vec::<String>::new());
    let project = Project::parse(MINIMAL, Path::new("."), options()).unwrap();
    assert_eq!(project.context.version(), "1.0.0");
}

#[test]
fn an_unknown_key_does_not_read() {
    let toml = format!("{MINIMAL}\n[macos.dmg]\nvolume = \"x\"\n");
    let error = Project::parse(&toml, Path::new("."), options()).unwrap_err();
    assert!(
        matches!(&error, Error::Config(text) if text.contains("volume")),
        "{error}"
    );
}

#[test]
fn every_problem_is_reported_at_once() {
    let toml = r#"
[app]
name = "Ex/ample"
id = "example"
version = "1.0.0"
url_schemes = ["example:"]

[vars]
version = "shadow"

[[document_type]]
name = "Doc"
extensions = [".doc", "doc"]

[[document_type]]
name = "Other"
extensions = ["DOC"]

[macos]
icon_name = "AppIcon"

[macos.executable]
from = []

[[macos.program]]
name = "helper"
from = "a"

[[macos.program]]
name = "helper"
from = "b"

[[macos.resource]]
from = "docs"
to = "../outside"

[[macos.extension]]
name = "Provider.appex"
bundle = "Provider.appex"
executable = "provider"

[[macos.launch_agent]]
label = "agent"

[macos.notarize]
staple = ["dmg"]

[windows.executable]
from = "a.exe"
name = "app"

[windows.nsis]
script = "installer.nsi"
update_flags = { update = "/U" }

[windows.registry]
open_command = '"{exe}"'

[windows.sign]
command = ["signer"]

[feed]
url = "http://updates.example.com/"
public_key = "RW..."

[variant.test]
id = "bad id"
"#;
    let found = problems_of(toml);
    let expect = [
        "app.name",
        "app.id",
        "app.url_schemes",
        "vars.version",
        "document_type[0].extensions",
        "claimed by two",
        "macos.executable.from lists no candidates",
        "is in Contents/MacOS twice",
        "macos.resource[0].to",
        "without .appex",
        "needs either bundle, or executable and info_plist",
        "macos.launch_agent[0] needs from, or program",
        "icon_name",
        "there is no [macos.dmg]",
        "must end in .exe",
        "update_flags.update",
        "open_command never passes %1",
        "never names {file}",
        "must be HTTPS",
        "variant.test.id",
    ];
    for needle in expect {
        assert!(
            found.iter().any(|problem| problem.contains(needle)),
            "no problem mentions {needle:?}:\n{}",
            found.join("\n")
        );
    }
    assert_eq!(found.len(), expect.len(), "{}", found.join("\n"));

    let error = Project::parse(toml, Path::new("."), options()).unwrap_err();
    assert!(matches!(error, Error::Invalid(list) if list.len() == expect.len()));
}

#[test]
fn nothing_to_package_is_a_problem() {
    let toml = "[app]\nname = \"E\"\nid = \"com.example.e\"\nversion = \"1.0.0\"\n";
    assert!(problems_of(toml)[0].contains("nothing to package"));
}
