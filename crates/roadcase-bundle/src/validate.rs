//! Checks a config beyond what its types say, and reports every problem at once.

use crate::config::{Config, Extension, LaunchAgent, Linux, Source, Variant};
use crate::context::BUILTINS;
use std::collections::BTreeSet;

/// Every problem in `config`, as sentences naming the key. Empty when it is fine.
pub fn problems(config: &Config) -> Vec<String> {
    let mut found = Vec::new();
    let mut problem = |text: String| found.push(text);

    let app = &config.app;
    if app.name.trim().is_empty() || app.name.contains(['/', '\\', ':']) {
        problem(format!(
            "app.name {:?} must be a file name: not empty, no / \\ or :",
            app.name
        ));
    }
    check_id("app.id", &app.id, &mut problem);
    for scheme in &app.url_schemes {
        check_scheme("app.url_schemes", scheme, &mut problem);
    }
    for (index, command) in app.build.iter().enumerate() {
        if command.run.is_empty() {
            problem(format!("app.build[{index}].run is empty"));
        }
    }
    for name in config.vars.keys() {
        check_var(&format!("vars.{name}"), name, &mut problem);
    }

    let mut extensions_seen = BTreeSet::new();
    for (index, document) in config.document_types.iter().enumerate() {
        let key = format!("document_type[{index}]");
        if document.name.trim().is_empty() {
            problem(format!("{key}.name is empty"));
        }
        if document.extensions.is_empty() && document.content_types.is_empty() {
            problem(format!("{key} has neither extensions nor content_types"));
        }
        for extension in &document.extensions {
            if extension.starts_with('.') || extension.is_empty() || extension.contains(['/', '\\'])
            {
                problem(format!(
                    "{key}.extensions: {extension:?} must be an extension without its dot"
                ));
            }
            if !extensions_seen.insert(extension.to_ascii_lowercase()) {
                problem(format!(
                    "{key}: .{extension} is claimed by two document types"
                ));
            }
        }
    }

    if let Some(macos) = &config.macos {
        let mut names = BTreeSet::new();
        names.insert(
            macos
                .executable
                .name
                .clone()
                .unwrap_or_else(|| app.name.clone()),
        );
        check_source(
            "macos.executable.from",
            &macos.executable.from,
            &mut problem,
        );
        if let Some(name) = &macos.executable.name {
            check_file_name("macos.executable.name", name, &mut problem);
        }
        for (index, program) in macos.programs.iter().enumerate() {
            let key = format!("macos.program[{index}]");
            check_file_name(&format!("{key}.name"), &program.name, &mut problem);
            check_source(&format!("{key}.from"), &program.from, &mut problem);
            if !names.insert(program.name.clone()) {
                problem(format!(
                    "{key}: {} is in Contents/MacOS twice",
                    program.name
                ));
            }
            if !program.sign && program.entitlements.is_some() {
                problem(format!("{key} has entitlements and sign = false"));
            }
        }
        for (index, resource) in macos.resources.iter().enumerate() {
            check_relative(
                &format!("macos.resource[{index}].to"),
                &resource.to,
                &mut problem,
            );
        }
        let mut appexes = BTreeSet::new();
        for (index, extension) in macos.extensions.iter().enumerate() {
            check_extension(index, extension, &mut problem);
            if !appexes.insert(extension.name.clone()) {
                problem(format!(
                    "macos.extension[{index}]: {}.appex is declared twice",
                    extension.name
                ));
            }
        }
        let mut labels = BTreeSet::new();
        for (index, agent) in macos.launch_agents.iter().enumerate() {
            check_agent(index, agent, &mut problem);
            if !labels.insert(agent.label.clone()) {
                problem(format!(
                    "macos.launch_agent[{index}]: {} is declared twice",
                    agent.label
                ));
            }
        }
        if macos.icon_name.is_some() && macos.assets_car.is_none() {
            problem("macos.icon_name names an icon in assets_car, which is not set".into());
        }
        if let Some(version) = &macos.min_system
            && !version
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|c| c.is_ascii_digit()))
        {
            problem(format!(
                "macos.min_system {version:?} is not a version such as 11.0"
            ));
        }
        for (key, declarations) in [
            ("macos.exported_type", &macos.exported_types),
            ("macos.imported_type", &macos.imported_types),
        ] {
            for (index, declaration) in declarations.iter().enumerate() {
                check_id(
                    &format!("{key}[{index}].identifier"),
                    &declaration.identifier,
                    &mut problem,
                );
            }
        }
        if let Some(notarize) = &macos.notarize
            && notarize.staple.is_empty()
        {
            problem("macos.notarize.staple is empty: drop [macos.notarize] instead".into());
        }
        if let Some(notarize) = &macos.notarize
            && notarize.staple.contains(&crate::config::Notarized::Dmg)
            && macos.dmg.is_none()
        {
            problem("macos.notarize.staple names the dmg, and there is no [macos.dmg]".into());
        }
    }

    if let Some(windows) = &config.windows {
        check_source(
            "windows.executable.from",
            &windows.executable.from,
            &mut problem,
        );
        let main = windows
            .executable
            .name
            .clone()
            .unwrap_or_else(|| format!("{}.exe", app.name));
        if !main.to_ascii_lowercase().ends_with(".exe") {
            problem(format!("windows.executable.name {main} must end in .exe"));
        }
        let mut names = BTreeSet::from([main.to_ascii_lowercase()]);
        for (index, program) in windows.programs.iter().enumerate() {
            let key = format!("windows.program[{index}]");
            check_file_name(&format!("{key}.name"), &program.name, &mut problem);
            check_source(&format!("{key}.from"), &program.from, &mut problem);
            if !names.insert(program.name.to_ascii_lowercase()) {
                problem(format!("{key}: {} is installed twice", program.name));
            }
        }
        for (index, resource) in windows.resources.iter().enumerate() {
            check_relative(
                &format!("windows.resource[{index}].to"),
                &resource.to,
                &mut problem,
            );
        }
        let flags = &windows.nsis.update_flags;
        for (key, given, ours) in [
            ("silent", &flags.silent, roadcase_update::NSIS_SILENT),
            ("update", &flags.update, roadcase_update::NSIS_UPDATE),
            ("relaunch", &flags.relaunch, roadcase_update::NSIS_RELAUNCH),
        ] {
            if given != ours {
                problem(format!(
                    "windows.nsis.update_flags.{key} is {given}, and the updater passes {ours}"
                ));
            }
        }
        for name in windows.nsis.defines.keys() {
            if name.is_empty() || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
                problem(format!(
                    "windows.nsis.defines.{name}: a define is letters, digits and _"
                ));
            }
        }
        for (key, command) in [
            (
                "windows.registry.open_command",
                Some(&windows.registry.open_command),
            ),
            (
                "windows.registry.url_command",
                windows.registry.url_command.as_ref(),
            ),
        ] {
            if let Some(command) = command
                && !command.contains("%1")
            {
                problem(format!("{key} never passes %1, the URL or file"));
            }
        }
        if let Some(sign) = &windows.sign {
            if sign.command.is_empty() {
                problem("windows.sign.command is empty".into());
            } else if !sign.command.iter().any(|part| part.contains("{file}")) {
                problem("windows.sign.command never names {file}".into());
            }
        }
    }

    if let Some(linux) = &config.linux {
        check_linux(config, linux, &mut problem);
    }

    if let Some(feed) = &config.feed
        && let crate::config::Value::Text(url) = &feed.url
        && !(url.starts_with("https://")
            || url.starts_with("http://127.0.0.1")
            || url.starts_with("http://localhost"))
    {
        problem(format!("feed.url {url} must be HTTPS"));
    }

    for (name, variant) in &config.variants {
        check_variant(name, variant, &mut problem);
    }

    if config.macos.is_none() && config.windows.is_none() && config.linux.is_none() {
        problem("there is no [macos], [windows] or [linux]: nothing to package".into());
    }
    found
}

fn check_id(key: &str, id: &str, problem: &mut impl FnMut(String)) {
    let parts: Vec<&str> = id.split('.').collect();
    let fine = parts.len() >= 2
        && parts.iter().all(|part| {
            !part.is_empty() && part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        });
    if !fine {
        problem(format!(
            "{key} {id:?} must be reverse DNS: letters, digits and - between dots"
        ));
    }
}

fn check_scheme(key: &str, scheme: &str, problem: &mut impl FnMut(String)) {
    let mut chars = scheme.chars();
    let fine = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !fine {
        problem(format!(
            "{key}: {scheme:?} is not a URL scheme (no colon: example, not example:)"
        ));
    }
}

fn check_var(key: &str, name: &str, problem: &mut impl FnMut(String)) {
    if BUILTINS.contains(&name) {
        problem(format!(
            "{key}: {{{name}}} is built in and cannot be redefined"
        ));
    }
    if name.is_empty()
        || name.starts_with("env:")
        || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
    {
        problem(format!("{key}: a var name is letters, digits and _"));
    }
}

fn check_file_name(key: &str, name: &str, problem: &mut impl FnMut(String)) {
    if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
        problem(format!("{key} {name:?} must be a file name"));
    }
}

fn check_relative(key: &str, path: &str, problem: &mut impl FnMut(String)) {
    let escapes = path.split(['/', '\\']).any(|part| part == "..");
    if path.starts_with('/') || path.starts_with('\\') || escapes {
        problem(format!(
            "{key} {path:?} must stay inside the bundle: relative, no .."
        ));
    }
}

fn check_source(key: &str, source: &Source, problem: &mut impl FnMut(String)) {
    if let Source::Candidates(candidates) = source
        && candidates.is_empty()
    {
        problem(format!("{key} lists no candidates"));
    }
}

fn check_extension(index: usize, extension: &Extension, problem: &mut impl FnMut(String)) {
    let key = format!("macos.extension[{index}]");
    check_file_name(&format!("{key}.name"), &extension.name, problem);
    if extension.name.ends_with(".appex") {
        problem(format!("{key}.name is the name without .appex"));
    }
    match (
        &extension.bundle,
        &extension.executable,
        &extension.info_plist,
    ) {
        (Some(_), None, None) => {
            if !extension.plist.is_empty() {
                problem(format!(
                    "{key}: plist keys are for an extension the packager assembles, not a finished bundle"
                ));
            }
        }
        (None, Some(source), Some(_)) => {
            check_source(&format!("{key}.executable"), source, problem)
        }
        _ => problem(format!(
            "{key} needs either bundle, or executable and info_plist"
        )),
    }
    if let Some(id) = &extension.id {
        check_id(&format!("{key}.id"), id, problem);
    }
}

fn check_agent(index: usize, agent: &LaunchAgent, problem: &mut impl FnMut(String)) {
    let key = format!("macos.launch_agent[{index}]");
    check_file_name(&format!("{key}.label"), &agent.label, problem);
    let generated = agent.program.is_some()
        || !agent.arguments.is_empty()
        || agent.run_at_load
        || agent.keep_alive;
    match (&agent.from, generated) {
        (Some(_), true) => problem(format!(
            "{key}: from copies a plist; program, arguments, run_at_load and keep_alive make one: not both"
        )),
        (None, false) => problem(format!("{key} needs from, or program")),
        (None, true) if agent.program.is_none() => problem(format!("{key} needs program")),
        _ => {}
    }
    if let Some(program) = &agent.program {
        check_relative(&format!("{key}.program"), program, problem);
    }
}

fn check_variant(name: &str, variant: &Variant, problem: &mut impl FnMut(String)) {
    let key = format!("variant.{name}");
    if let Some(id) = &variant.id {
        check_id(&format!("{key}.id"), id, problem);
    }
    if let Some(schemes) = &variant.url_schemes {
        for scheme in schemes {
            check_scheme(&format!("{key}.url_schemes"), scheme, problem);
        }
    }
    for var in variant.vars.keys() {
        check_var(&format!("{key}.vars.{var}"), var, problem);
    }
}

/// A package's own release number: `1`, `2`, `1.1`.
fn check_release(key: &str, value: &str, problem: &mut impl FnMut(String)) {
    let fine = !value.is_empty()
        && !value.starts_with('.')
        && !value.ends_with('.')
        && value.bytes().all(|c| c.is_ascii_digit() || c == b'.');
    if !fine && !value.contains('{') {
        problem(format!("{key} {value:?} is not a release number such as 1"));
    }
}

fn check_linux(config: &Config, linux: &Linux, problem: &mut impl FnMut(String)) {
    check_source("linux.executable.from", &linux.executable.from, problem);
    let main = match &linux.executable.name {
        Some(name) => {
            check_file_name("linux.executable.name", name, problem);
            name.clone()
        }
        None => crate::linux::default_executable_name(&config.app.name),
    };
    let mut names = BTreeSet::from([main]);
    for (index, program) in linux.programs.iter().enumerate() {
        let key = format!("linux.program[{index}]");
        check_file_name(&format!("{key}.name"), &program.name, problem);
        check_source(&format!("{key}.from"), &program.from, problem);
        if !names.insert(program.name.clone()) {
            problem(format!("{key}: {} is in usr/bin twice", program.name));
        }
    }
    for (index, resource) in linux.resources.iter().enumerate() {
        check_relative(
            &format!("linux.resource[{index}].to"),
            &resource.to,
            problem,
        );
    }
    if linux.icons.is_empty() {
        problem("linux.icons is empty: an AppImage needs an icon".into());
    }
    for icon in &linux.icons {
        let extension = icon
            .extension()
            .map(|extension| extension.to_string_lossy().to_ascii_lowercase());
        if !matches!(extension.as_deref(), Some("png" | "svg")) {
            problem(format!(
                "linux.icons: {} is neither a .png nor an .svg",
                icon.display()
            ));
        }
    }
    if linux.categories.is_empty() {
        problem("linux.categories is empty: a desktop entry needs one, such as Utility".into());
    }
    if let Some(arguments) = &linux.exec_arguments {
        let codes = arguments
            .iter()
            .filter(|argument| crate::linux::FILE_FIELD_CODES.contains(&argument.as_str()))
            .count();
        if codes > 1 {
            problem("linux.exec_arguments holds more than one of %f, %F, %u and %U".into());
        }
    }
    for key in linux.desktop.keys() {
        if crate::linux::GENERATED_KEYS.contains(&key.as_str()) {
            problem(format!(
                "linux.desktop.{key} is written from the rest of the config"
            ));
        }
        if key.is_empty() || !key.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
            problem(format!(
                "linux.desktop.{key}: a key is letters, digits and -"
            ));
        }
    }
    if let Some(appimage) = &linux.appimage {
        check_file_name("linux.appimage.name", &appimage.name, problem);
    }
    let executable = linux
        .executable
        .name
        .clone()
        .unwrap_or_else(|| crate::linux::default_executable_name(&config.app.name));
    if let Some(deb) = &linux.deb {
        let package = deb.package.as_deref().unwrap_or(&executable);
        let fine = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit() || b"+-.".contains(&c);
        if !package.contains('{')
            && (package.len() < 2
                || !package.bytes().all(fine)
                || !package.as_bytes()[0].is_ascii_alphanumeric())
        {
            problem(format!(
                "linux.deb.package {package:?} is not a Debian package name: lower-case letters, \
                 digits and +-., two or more, starting with a letter or digit (set it when the \
                 executable's name is not one)"
            ));
        }
        if let Some(name) = &deb.name {
            check_file_name("linux.deb.name", name, problem);
        }
        check_release("linux.deb.revision", &deb.revision, problem);
        if deb.maintainer.is_none() && config.app.publisher.is_none() {
            problem(
                "linux.deb.maintainer (or app.publisher) is needed: Example <packages@example.com>"
                    .into(),
            );
        }
        if let Some(source) = &deb.apt_source {
            if !source.url.contains('{')
                && !source.url.starts_with("https://")
                && !source.url.starts_with("http://")
            {
                problem(format!(
                    "linux.deb.apt_source.url {:?} is not an http(s) URL",
                    source.url
                ));
            }
            for (key, value) in std::iter::once(("suite", &source.suite)).chain(
                source
                    .components
                    .iter()
                    .map(|component| ("components", component)),
            ) {
                if value.is_empty()
                    || !value
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
                {
                    problem(format!(
                        "linux.deb.apt_source.{key}: {value:?} is not a plain name"
                    ));
                }
            }
            if source.components.is_empty() {
                problem("linux.deb.apt_source.components is empty".into());
            }
        }
    }
    if let Some(pacman) = &linux.pacman {
        let package = pacman.pkgname.as_deref().unwrap_or(&executable);
        let fine = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit() || b"@._+-".contains(&c);
        if !package.contains('{')
            && (package.is_empty() || !package.bytes().all(fine) || package.starts_with(['-', '.']))
        {
            problem(format!(
                "linux.pacman.pkgname {package:?} is not a pacman package name: lower-case \
                 letters, digits and @._+-, not starting with - or . (set it when the \
                 executable's name is not one)"
            ));
        }
        check_release("linux.pacman.pkgrel", &pacman.pkgrel, problem);
    }
    for (index, document) in config.document_types.iter().enumerate() {
        let key = format!("document_type[{index}]");
        match &document.mime_type {
            Some(mime) => {
                let fine = mime.split('/').count() == 2
                    && mime.split('/').all(|part| {
                        !part.is_empty()
                            && part
                                .bytes()
                                .all(|c| c.is_ascii_alphanumeric() || b"+-._".contains(&c))
                    });
                if !fine {
                    problem(format!(
                        "{key}.mime_type {mime:?} is not a MIME type such as application/x-example"
                    ));
                }
            }
            None if document.extensions.is_empty() => problem(format!(
                "{key} has no extensions, so Linux needs its mime_type"
            )),
            None => {}
        }
    }
}
