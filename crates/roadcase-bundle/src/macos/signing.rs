//! Signing a bundle inside out with `codesign`: who signs (ad hoc, or a
//! certificate whose name says whether it is a Developer ID and whose
//! organizational unit is the team), each step's command, and a temporary
//! keychain for a certificate that arrives as a base64 `.p12`.

use super::{AppBundle, SignStep, render_entitlements};
use crate::config::Toggle;
use crate::error::io;
use crate::process::Invocation;
use crate::{Error, Project, Result};
use std::path::{Path, PathBuf};

const CODESIGN: &str = "/usr/bin/codesign";
const SECURITY: &str = "/usr/bin/security";
const OPENSSL: &str = "/usr/bin/openssl";

/// Who signs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// What `codesign --sign` is given: a name, a SHA-1 hash, or `-`.
    pub name: String,
    /// The only keychain it is looked up in.
    pub keychain: Option<String>,
    /// Whether its certificate is a Developer ID, which notarization needs.
    pub developer_id: bool,
    /// The Apple team of its certificate.
    pub team: Option<String>,
}

impl Identity {
    /// The ad hoc identity: a signature that names nobody, for this machine.
    pub fn ad_hoc() -> Self {
        Self {
            name: "-".into(),
            keychain: None,
            developer_id: false,
            team: None,
        }
    }

    /// Whether it signs ad hoc.
    pub fn is_ad_hoc(&self) -> bool {
        self.name == "-"
    }
}

/// The common name and the organizational unit in what
/// `openssl x509 -noout -subject -nameopt multiline` prints.
pub fn parse_subject(text: &str) -> (Option<String>, Option<String>) {
    let field = |name: &str| {
        text.lines().find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == name).then(|| value.trim().to_owned())
        })
    };
    (field("commonName"), field("organizationalUnitName"))
}

/// The certificate with this SHA-1 hash in what
/// `security find-certificate -a -Z -p` prints.
pub fn certificate_with_hash(listing: &str, hash: &str) -> Option<String> {
    let mut take = false;
    let mut pem = String::new();
    for line in listing.lines() {
        if let Some(found) = line.strip_prefix("SHA-1 hash:") {
            if take && !pem.is_empty() {
                break;
            }
            take = found.trim().eq_ignore_ascii_case(hash);
        } else if line.starts_with("SHA-256 hash:") {
            continue;
        } else if take {
            pem.push_str(line);
            pem.push('\n');
        }
    }
    (!pem.is_empty()).then_some(pem)
}

/// The SHA-1 hashes in what `security find-identity -v` prints.
pub fn identity_hashes(listing: &str) -> Vec<String> {
    listing
        .lines()
        .filter_map(|line| {
            let (_, rest) = line.trim().split_once(") ")?;
            let hash = rest.split_whitespace().next()?;
            (hash.len() == 40 && hash.bytes().all(|c| c.is_ascii_hexdigit()))
                .then(|| hash.to_owned())
        })
        .collect()
}

fn is_hash(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(|c| c.is_ascii_hexdigit())
}

fn is_team(text: &str) -> bool {
    text.len() == 10
        && text
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// Runs a command with `input` on its standard input.
fn piped(program: &str, arguments: &[&str], input: &str) -> Result<String> {
    use std::io::Write;
    use std::process::Stdio;
    let failed = |error: std::io::Error| Error::Command(format!("{program}: {error}"));
    let mut child = std::process::Command::new(program)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(failed)?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes()).map_err(failed)?;
    }
    let output = child.wait_with_output().map_err(failed)?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(Error::Command(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// Resolves `[macos.signing]`: the identity, the keychain, and from the
/// identity's certificate whether it is a Developer ID and which team it
/// belongs to. `keychain` replaces the configured keychain (a temporary one
/// that holds an imported certificate); the default identity (`-`) is then
/// that keychain's first.
///
/// # Errors
///
/// [`Error::Config`] when no certificate matches the identity;
/// [`Error::Command`]; [`Error::Value`].
pub fn identity(project: &Project, keychain: Option<&Path>) -> Result<Identity> {
    let signing = &project.macos()?.signing;
    let context = &project.context;
    let mut name = context.resolve(&signing.identity, "macos.signing.identity")?;
    let configured = match &signing.keychain {
        Some(value) => Some(context.resolve(value, "macos.signing.keychain")?),
        None => None,
    }
    .filter(|keychain| !keychain.is_empty());
    let imported = keychain.map(|path| path.to_string_lossy().into_owned());
    let keychain = imported.clone().or(configured);

    if name == "-" || name.is_empty() {
        // Only an imported certificate changes what the default means.
        let Some(imported) = imported else {
            return Ok(Identity::ad_hoc());
        };
        let listing = Invocation::new(SECURITY)
            .args(["find-identity", "-v", "-p", "codesigning", &imported])
            .run()?;
        name = identity_hashes(&listing)
            .into_iter()
            .next()
            .ok_or_else(|| {
                Error::Config(format!(
                    "macos.signing.certificate holds no valid code signing identity ({imported})"
                ))
            })?;
    }

    let mut find = Invocation::new(SECURITY).arg("find-certificate");
    find = if is_hash(&name) {
        find.args(["-a", "-Z", "-p"])
    } else {
        find.args(["-c", &name, "-p"])
    };
    if let Some(keychain) = &keychain {
        find = find.arg(keychain.clone());
    }
    let missing = || {
        Error::Config(format!(
            "macos.signing.identity: no certificate for {name}{}",
            keychain
                .as_ref()
                .map(|keychain| format!(" in {keychain}"))
                .unwrap_or_default()
        ))
    };
    let listing = find.run().map_err(|_| missing())?;
    let certificate = if is_hash(&name) {
        certificate_with_hash(&listing, &name).ok_or_else(missing)?
    } else {
        listing
    };
    if !certificate.contains("BEGIN CERTIFICATE") {
        return Err(missing());
    }
    let subject = piped(
        OPENSSL,
        &["x509", "-noout", "-subject", "-nameopt", "multiline"],
        &certificate,
    )?;
    let (common_name, unit) = parse_subject(&subject);
    Ok(Identity {
        developer_id: common_name.is_some_and(|name| name.starts_with("Developer ID")),
        team: unit.filter(|unit| is_team(unit)),
        name,
        keychain,
    })
}

/// `[macos.signing] certificate` and its password, when the certificate has a
/// value.
///
/// # Errors
///
/// [`Error::Value`].
pub fn signing_certificate(project: &Project) -> Result<Option<(String, String)>> {
    let signing = &project.macos()?.signing;
    let context = &project.context;
    let certificate = match &signing.certificate {
        Some(value) => context.resolve(value, "macos.signing.certificate")?,
        None => String::new(),
    };
    if certificate.trim().is_empty() {
        return Ok(None);
    }
    let password = match &signing.certificate_password {
        Some(value) => context.resolve(value, "macos.signing.certificate_password")?,
        None => String::new(),
    };
    Ok(Some((certificate, password)))
}

/// What a release signs with: the identity, and the temporary keychain that
/// holds its certificate when `[macos.signing] certificate` gave one. The
/// keychain is removed when this is dropped, or by [`Signing::finish`], which
/// says when that fails.
#[derive(Debug)]
pub struct Signing {
    /// Who signs.
    pub identity: Identity,
    keychain: Option<TemporaryKeychain>,
}

impl Signing {
    /// Imports the certificate if the config has one, resolves the identity,
    /// and gives the project its team (`{team}`) when the config did not.
    /// Run it before the build, whose commands may use `{team}`.
    ///
    /// # Errors
    ///
    /// As [`identity`] and [`TemporaryKeychain::import`].
    pub fn prepare(project: &mut Project) -> Result<Self> {
        let keychain = match signing_certificate(project)? {
            Some((certificate, password)) => Some(TemporaryKeychain::import(
                &certificate,
                &password,
                &TemporaryKeychain::default_path(),
            )?),
            None => None,
        };
        let identity = identity(project, keychain.as_ref().map(TemporaryKeychain::path))?;
        if project.context.get("team").is_none()
            && let Some(team) = &identity.team
        {
            project.context.set("team", team.clone());
        }
        Ok(Self { identity, keychain })
    }

    /// Signing ad hoc, with no keychain.
    pub fn ad_hoc() -> Self {
        Self {
            identity: Identity::ad_hoc(),
            keychain: None,
        }
    }

    /// Removes the temporary keychain, if there is one.
    ///
    /// # Errors
    ///
    /// [`Error::Command`] when `security` cannot remove it.
    pub fn finish(mut self) -> Result<()> {
        match self.keychain.take() {
            Some(keychain) => keychain.remove(),
            None => Ok(()),
        }
    }
}

/// Whether the hardened runtime and a secure timestamp are asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignFlags {
    /// `--options runtime`.
    pub hardened_runtime: bool,
    /// `Some(true)` is `--timestamp`, `Some(false)` is `--timestamp=none`,
    /// `None` leaves it to `codesign`.
    pub timestamp: Option<bool>,
}

impl SignFlags {
    /// What the toggles mean for this identity: `auto` is on for a
    /// Developer ID.
    pub fn new(hardened_runtime: Toggle, timestamp: Toggle, identity: &Identity) -> Self {
        Self {
            hardened_runtime: match hardened_runtime {
                Toggle::Auto => identity.developer_id,
                Toggle::Always => true,
                Toggle::Never => false,
            },
            timestamp: match timestamp {
                Toggle::Auto => identity.developer_id.then_some(true),
                Toggle::Always => Some(true),
                Toggle::Never => Some(false),
            },
        }
    }

    /// As `[macos.signing]` says, for this identity.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] without `[macos]`.
    pub fn of(project: &Project, identity: &Identity) -> Result<Self> {
        let signing = &project.macos()?.signing;
        Ok(Self::new(
            signing.hardened_runtime,
            signing.timestamp,
            identity,
        ))
    }
}

/// The `codesign` command for one step. `--deep` is never used to sign.
pub fn codesign_command(identity: &Identity, flags: SignFlags, step: &SignStep) -> Invocation {
    let mut command = Invocation::new(CODESIGN).arg("--force");
    if flags.hardened_runtime {
        command = command.args(["--options", "runtime"]);
    }
    match flags.timestamp {
        Some(true) => command = command.arg("--timestamp"),
        Some(false) => command = command.arg("--timestamp=none"),
        None => {}
    }
    command = command.args(["--sign", &identity.name]);
    if let Some(keychain) = &identity.keychain {
        command = command.args(["--keychain", keychain]);
    }
    if let Some(identifier) = &step.identifier {
        command = command.args(["--identifier", identifier]);
    }
    if let Some(entitlements) = &step.entitlements {
        command = command.arg("--entitlements").path(entitlements);
    }
    command.path(&step.path)
}

/// The command that checks a signed bundle, nested code included.
pub fn verify_command(path: &Path) -> Invocation {
    Invocation::new(CODESIGN)
        .args(["--verify", "--deep", "--strict"])
        .path(path)
}

/// Signs the bundle as [`AppBundle::signing`] says: nested code first, each
/// piece with its own entitlements (placeholders filled, written beside the
/// bundle in `<name>.entitlements/`) and identifier, the bundle last; then
/// checks it with `codesign --verify --deep --strict`.
///
/// # Errors
///
/// [`Error::Value`] when an entitlement's placeholder (often `{team}`) cannot
/// be filled; [`Error::Command`] when `codesign` fails.
pub fn sign(project: &Project, app: &AppBundle, identity: &Identity) -> Result<()> {
    let flags = SignFlags::of(project, identity)?;
    let folder = app.path.with_extension("entitlements");
    if folder.exists() {
        std::fs::remove_dir_all(&folder).map_err(io(&folder))?;
    }
    for step in render_entitlements(project, &app.signing, &folder)? {
        codesign_command(identity, flags, &step).run_all()?;
    }
    verify_command(&app.path).run_all()?;
    Ok(())
}

/// The command that signs a disk image: no entitlements, no runtime.
pub fn sign_file_command(identity: &Identity, flags: SignFlags, path: &Path) -> Invocation {
    codesign_command(
        identity,
        SignFlags {
            hardened_runtime: false,
            ..flags
        },
        &SignStep {
            path: path.to_path_buf(),
            entitlements: None,
            identifier: None,
        },
    )
}

/// A keychain made for one run, holding a certificate and its private key
/// that came as a base64 `.p12`. It is removed when dropped.
#[derive(Debug)]
pub struct TemporaryKeychain {
    path: PathBuf,
    removed: bool,
}

impl TemporaryKeychain {
    /// Where one goes by default: `$RUNNER_TEMP`, or the system's temporary
    /// folder, named after this process.
    pub fn default_path() -> PathBuf {
        let folder = std::env::var_os("RUNNER_TEMP")
            .map(PathBuf::from)
            .filter(|folder| folder.is_dir())
            .unwrap_or_else(std::env::temp_dir);
        folder.join(format!("roadcase-{}.keychain-db", std::process::id()))
    }

    /// Makes the keychain at `path` with a random password, imports the
    /// `.p12` (base64, as a secret store holds it) for `codesign` to use
    /// without asking, and adds the keychain to the person's search list.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] when the certificate is not base64 or a keychain is
    /// already at `path`; [`Error::Command`] when `security` fails;
    /// [`Error::Io`].
    pub fn import(p12_base64: &str, p12_password: &str, path: &Path) -> Result<Self> {
        let p12 = decode_base64(p12_base64).ok_or_else(|| {
            Error::Config("the signing certificate is not a base64 .p12".to_owned())
        })?;
        if path.exists() {
            return Err(Error::Config(format!(
                "{} exists: remove it with `roadcase keychain remove`",
                path.display()
            )));
        }
        let password = random_password()?;
        let file = path.with_extension("p12");
        write_private(&file, &p12)?;
        let existing = Invocation::new(SECURITY)
            .args(["list-keychains", "-d", "user"])
            .run()
            .map(|listing| parse_keychain_list(&listing))
            .unwrap_or_default();
        // From here on the keychain exists: dropping this removes it.
        let made = Self {
            path: path.to_path_buf(),
            removed: false,
        };
        let done = keychain_commands(path, &password, &file, p12_password, &existing)
            .iter()
            .try_for_each(|command| command.run_all().map(|_| ()));
        let _ = std::fs::remove_file(&file);
        done.map(|()| made)
    }

    /// The keychain's file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Removes the keychain, which also takes it off the search list.
    ///
    /// # Errors
    ///
    /// [`Error::Command`] when `security` cannot.
    pub fn remove(mut self) -> Result<()> {
        self.removed = true;
        remove_keychain(&self.path)
    }
}

impl Drop for TemporaryKeychain {
    fn drop(&mut self) {
        if !self.removed {
            let _ = remove_keychain(&self.path);
        }
    }
}

/// Removes a keychain made by [`TemporaryKeychain::import`] (or by
/// `roadcase keychain import`), and takes it off the search list.
///
/// # Errors
///
/// [`Error::Command`] when `security` cannot.
pub fn remove_keychain(path: &Path) -> Result<()> {
    let _ = std::fs::remove_file(path.with_extension("p12"));
    Invocation::new(SECURITY)
        .arg("delete-keychain")
        .path(path)
        .run_all()
        .map(|_| ())
}

/// What `security` runs to make a keychain that `codesign` can sign from
/// with no prompt: create, never lock during the run, unlock, import the
/// `.p12` for `codesign`, allow Apple's tools to use the key, and put the
/// keychain first in the search list, before those already there.
pub fn keychain_commands(
    keychain: &Path,
    keychain_password: &str,
    p12: &Path,
    p12_password: &str,
    existing: &[String],
) -> Vec<Invocation> {
    let security = || Invocation::new(SECURITY);
    let here = keychain.to_string_lossy().into_owned();
    vec![
        security()
            .args(["create-keychain", "-p"])
            .secret(keychain_password)
            .path(keychain),
        security()
            .args(["set-keychain-settings", "-lut", "21600"])
            .path(keychain),
        security()
            .args(["unlock-keychain", "-p"])
            .secret(keychain_password)
            .path(keychain),
        security()
            .arg("import")
            .path(p12)
            .arg("-k")
            .path(keychain)
            .args(["-f", "pkcs12", "-P"])
            .secret(p12_password)
            .args(["-T", CODESIGN, "-T", SECURITY]),
        security()
            .args([
                "set-key-partition-list",
                "-S",
                "apple-tool:,apple:,codesign:",
                "-s",
                "-k",
            ])
            .secret(keychain_password)
            .path(keychain),
        security()
            .args(["list-keychains", "-d", "user", "-s"])
            .path(keychain)
            .args(existing.iter().filter(|other| **other != here).cloned()),
    ]
}

/// The paths in what `security list-keychains` prints: one per line, in
/// quotes.
pub fn parse_keychain_list(listing: &str) -> Vec<String> {
    listing
        .lines()
        .map(|line| line.trim().trim_matches('"').to_owned())
        .filter(|path| !path.is_empty())
        .collect()
}

fn random_password() -> Result<String> {
    use std::io::Read;
    let source = Path::new("/dev/urandom");
    let mut bytes = [0u8; 24];
    std::fs::File::open(source)
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(io(source))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Writes a file only its owner can read.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    options
        .open(path)
        .and_then(|mut file| file.write_all(bytes))
        .map_err(io(path))
}

/// Decodes standard base64, ignoring white space and missing padding.
pub fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    let mut ended = false;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => {
                ended = true;
                continue;
            }
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        if ended {
            return None;
        }
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn developer() -> Identity {
        Identity {
            name: "Developer ID Application: Example Inc. (ABCDE12345)".into(),
            keychain: Some("/tmp/build.keychain-db".into()),
            developer_id: true,
            team: Some("ABCDE12345".into()),
        }
    }

    fn step(entitlements: Option<&str>, identifier: Option<&str>) -> SignStep {
        SignStep {
            path: "/out/Example.app/Contents/MacOS/helper".into(),
            entitlements: entitlements.map(PathBuf::from),
            identifier: identifier.map(str::to_owned),
        }
    }

    #[test]
    fn a_developer_id_signs_with_the_runtime_and_a_timestamp() {
        let identity = developer();
        let flags = SignFlags::new(Toggle::Auto, Toggle::Auto, &identity);
        let command = codesign_command(
            &identity,
            flags,
            &step(Some("/out/helper.entitlements"), Some("com.example.app")),
        );
        assert_eq!(command.program, "/usr/bin/codesign");
        assert_eq!(
            command.arguments,
            [
                "--force",
                "--options",
                "runtime",
                "--timestamp",
                "--sign",
                "Developer ID Application: Example Inc. (ABCDE12345)",
                "--keychain",
                "/tmp/build.keychain-db",
                "--identifier",
                "com.example.app",
                "--entitlements",
                "/out/helper.entitlements",
                "/out/Example.app/Contents/MacOS/helper",
            ]
        );
    }

    #[test]
    fn an_ad_hoc_signature_has_neither() {
        let identity = Identity::ad_hoc();
        let flags = SignFlags::new(Toggle::Auto, Toggle::Auto, &identity);
        assert_eq!(
            codesign_command(&identity, flags, &step(None, None)).arguments,
            [
                "--force",
                "--sign",
                "-",
                "/out/Example.app/Contents/MacOS/helper"
            ]
        );
        let forced = SignFlags::new(Toggle::Always, Toggle::Never, &identity);
        assert_eq!(
            codesign_command(&identity, forced, &step(None, None)).arguments[1..4],
            ["--options", "runtime", "--timestamp=none"]
        );
    }

    #[test]
    fn a_disk_image_is_signed_without_the_runtime() {
        let identity = developer();
        let flags = SignFlags::new(Toggle::Auto, Toggle::Auto, &identity);
        let command = sign_file_command(&identity, flags, Path::new("/out/Example.dmg"));
        assert!(!command.arguments.contains(&"runtime".to_owned()));
        assert!(command.arguments.contains(&"--timestamp".to_owned()));
        assert_eq!(command.arguments.last().unwrap(), "/out/Example.dmg");
    }

    #[test]
    fn a_certificate_subject_gives_the_name_and_the_team() {
        let subject = "subject=\n    userId                    = ABCDE12345\n    commonName                = Developer ID Application: Example Inc. (ABCDE12345)\n    organizationalUnitName    = ABCDE12345\n    organizationName          = Example Inc.\n    countryName               = CA\n";
        let (name, unit) = parse_subject(subject);
        assert_eq!(
            name.as_deref(),
            Some("Developer ID Application: Example Inc. (ABCDE12345)")
        );
        assert_eq!(unit.as_deref(), Some("ABCDE12345"));
        assert!(is_team("ABCDE12345"));
        assert!(!is_team("Engineering"));
    }

    #[test]
    fn a_certificate_is_found_by_its_hash() {
        let listing = "SHA-256 hash: AA\nSHA-1 hash: 1111111111111111111111111111111111111111\n-----BEGIN CERTIFICATE-----\nfirst\n-----END CERTIFICATE-----\nSHA-256 hash: BB\nSHA-1 hash: 2222222222222222222222222222222222222222\n-----BEGIN CERTIFICATE-----\nsecond\n-----END CERTIFICATE-----\n";
        let found =
            certificate_with_hash(listing, "2222222222222222222222222222222222222222").unwrap();
        assert!(found.contains("second") && !found.contains("first"));
        assert!(
            certificate_with_hash(listing, "3333333333333333333333333333333333333333").is_none()
        );
    }

    #[test]
    fn identities_are_read_from_the_listing() {
        let listing = "  1) B9613CE482E0F48E1190E7265CFB493DCC48FA46 \"Developer ID Application: Example Inc. (ABCDE12345)\"\n     1 valid identities found\n";
        assert_eq!(
            identity_hashes(listing),
            ["B9613CE482E0F48E1190E7265CFB493DCC48FA46"]
        );
    }

    #[test]
    fn a_keychain_is_made_for_codesign_and_put_first() {
        let commands = keychain_commands(
            Path::new("/tmp/r.keychain-db"),
            "kpw",
            Path::new("/tmp/r.p12"),
            "ppw",
            &[
                "/Users/me/Library/Keychains/login.keychain-db".into(),
                "/tmp/r.keychain-db".into(),
            ],
        );
        let lines: Vec<String> = commands.iter().map(Invocation::display).collect();
        assert_eq!(
            lines,
            [
                "/usr/bin/security create-keychain -p *** /tmp/r.keychain-db",
                "/usr/bin/security set-keychain-settings -lut 21600 /tmp/r.keychain-db",
                "/usr/bin/security unlock-keychain -p *** /tmp/r.keychain-db",
                "/usr/bin/security import /tmp/r.p12 -k /tmp/r.keychain-db -f pkcs12 -P *** -T /usr/bin/codesign -T /usr/bin/security",
                "/usr/bin/security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k *** /tmp/r.keychain-db",
                "/usr/bin/security list-keychains -d user -s /tmp/r.keychain-db /Users/me/Library/Keychains/login.keychain-db",
            ]
        );
        assert_eq!(
            parse_keychain_list("    \"/Users/me/Library/Keychains/login.keychain-db\"\n"),
            ["/Users/me/Library/Keychains/login.keychain-db"]
        );
    }

    #[test]
    fn base64_decodes_with_line_breaks() {
        assert_eq!(decode_base64("aGVs\nbG8=").unwrap(), b"hello");
        assert_eq!(decode_base64("aGVsbG8").unwrap(), b"hello");
        assert!(decode_base64("not base64!").is_none());
        assert!(decode_base64("").is_none());
    }
}
