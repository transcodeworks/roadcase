//! A `.deb`'s control file, read without dpkg.

use crate::archive::{ar_members, decompress, tar_entries};
use crate::error::io;
use crate::{Error, Result};
use std::path::Path;

/// A control file (deb822 paragraph): its fields in order. A value keeps its
/// continuation lines, each after a `\n` and starting with a space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Control {
    /// The fields, as written.
    pub fields: Vec<(String, String)>,
}

impl Control {
    /// Reads one paragraph.
    ///
    /// # Errors
    ///
    /// [`Error::Package`] for a line that is neither a field nor a
    /// continuation.
    pub fn parse(text: &str) -> Result<Self> {
        let mut fields: Vec<(String, String)> = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                if fields.is_empty() {
                    continue;
                }
                break;
            }
            if line.starts_with([' ', '\t']) {
                let (_, value) = fields.last_mut().ok_or_else(|| {
                    Error::Package("the control file starts with a continuation".into())
                })?;
                value.push('\n');
                value.push_str(line);
            } else {
                let (key, value) = line.split_once(':').ok_or_else(|| {
                    Error::Package(format!("the control line {line:?} is not a field"))
                })?;
                fields.push((key.trim().to_owned(), value.trim().to_owned()));
            }
        }
        Ok(Self { fields })
    }

    /// A field's value, by name in any case.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.as_str())
    }

    fn required(&self, key: &str) -> Result<&str> {
        self.get(key)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::Package(format!("the control file has no {key}")))
    }

    /// `Package`.
    ///
    /// # Errors
    ///
    /// [`Error::Package`] when it is missing.
    pub fn package(&self) -> Result<&str> {
        self.required("Package")
    }

    /// `Version`.
    ///
    /// # Errors
    ///
    /// [`Error::Package`] when it is missing.
    pub fn version(&self) -> Result<&str> {
        self.required("Version")
    }

    /// `Architecture`.
    ///
    /// # Errors
    ///
    /// [`Error::Package`] when it is missing.
    pub fn architecture(&self) -> Result<&str> {
        self.required("Architecture")
    }

    /// The paragraph as text, each field on its line.
    pub fn write(&self) -> String {
        self.fields
            .iter()
            .map(|(key, value)| {
                if value.is_empty() {
                    format!("{key}:\n")
                } else if value.starts_with('\n') {
                    format!("{key}:{value}\n")
                } else {
                    format!("{key}: {value}\n")
                }
            })
            .collect()
    }
}

/// Reads the control file of the `.deb` at `path`.
///
/// # Errors
///
/// [`Error::Io`]; [`Error::Package`] when it is not a `.deb` or its control
/// file does not read.
pub fn read_control(path: &Path) -> Result<Control> {
    let bytes = std::fs::read(path).map_err(io(path))?;
    control_of(&bytes).map_err(|error| match error {
        Error::Package(message) => Error::Package(format!("{}: {message}", path.display())),
        other => other,
    })
}

/// The control file of a `.deb`'s bytes.
pub(crate) fn control_of(bytes: &[u8]) -> Result<Control> {
    let members = ar_members(bytes)?;
    match members.first() {
        Some((name, version)) if name == "debian-binary" && version.starts_with(b"2.") => {}
        _ => return Err(Error::Package("it is not a Debian package".into())),
    }
    let (_, control) = members
        .iter()
        .find(|(name, _)| name.starts_with("control.tar"))
        .ok_or_else(|| Error::Package("it has no control.tar".into()))?;
    let tar = decompress(control)?;
    let entries = tar_entries(&tar, |path| path == "control")?;
    let control = entries
        .into_iter()
        .find_map(|entry| entry.contents.filter(|_| entry.path == "control"))
        .ok_or_else(|| Error::Package("its control.tar has no control file".into()))?;
    Control::parse(&String::from_utf8_lossy(&control))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A `.deb` holding `control`, with an empty data.tar.
    pub(crate) fn deb(control: &str) -> Vec<u8> {
        let tar = |files: &[(&str, &[u8])]| {
            let mut builder = tar::Builder::new(Vec::new());
            for (name, data) in files {
                let mut header = tar::Header::new_gnu();
                header.set_size(data.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder.append_data(&mut header, name, *data).unwrap();
            }
            builder.into_inner().unwrap()
        };
        let gzip = |bytes: &[u8]| {
            use std::io::Write;
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            encoder.write_all(bytes).unwrap();
            encoder.finish().unwrap()
        };
        let control = gzip(&tar(&[("./control", control.as_bytes())]));
        let data = gzip(&tar(&[]));
        let mut ar = b"!<arch>\n".to_vec();
        for (name, member) in [
            ("debian-binary", &b"2.0\n"[..]),
            ("control.tar.gz", &control),
            ("data.tar.gz", &data),
        ] {
            ar.extend_from_slice(
                format!(
                    "{name:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
                    0,
                    0,
                    0,
                    100644,
                    member.len()
                )
                .as_bytes(),
            );
            ar.extend_from_slice(member);
            if member.len() % 2 == 1 {
                ar.push(b'\n');
            }
        }
        ar
    }

    #[test]
    fn a_control_file_reads_and_writes_back() {
        let text = "Package: example\nVersion: 1.0.0-1\nArchitecture: amd64\nDescription: An example\n Longer.\n .\n More.\n";
        let control = Control::parse(text).unwrap();
        assert_eq!(control.package().unwrap(), "example");
        assert_eq!(control.get("version"), Some("1.0.0-1"));
        assert_eq!(
            control.get("Description"),
            Some("An example\n Longer.\n .\n More.")
        );
        assert_eq!(control.write(), text);
        assert!(Control::parse(" starts badly").is_err());
        assert!(Control::parse("no colon").is_err());
    }

    #[test]
    fn a_deb_gives_its_control_file() {
        let bytes = deb("Package: example\nVersion: 2.0.0-1\nArchitecture: arm64\n");
        let control = control_of(&bytes).unwrap();
        assert_eq!(control.version().unwrap(), "2.0.0-1");
        assert_eq!(control.architecture().unwrap(), "arm64");
        assert!(control_of(b"!<arch>\n").is_err());
    }
}
