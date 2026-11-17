//! Reading what packages are made of: `ar` archives (a `.deb`), and tar
//! archives compressed with gzip, xz or zstd, or not at all.

use crate::{Error, Result};
use std::io::Read;

fn package(error: impl std::fmt::Display) -> Error {
    Error::Package(error.to_string())
}

/// The members of an `ar` archive, by name, in order.
pub(crate) fn ar_members(bytes: &[u8]) -> Result<Vec<(String, &[u8])>> {
    let mut rest = bytes
        .strip_prefix(b"!<arch>\n")
        .ok_or_else(|| package("it is not an ar archive"))?;
    let mut members = Vec::new();
    while !rest.is_empty() {
        if rest.len() < 60 {
            return Err(package("an ar header is cut short"));
        }
        let (header, after) = rest.split_at(60);
        let name = String::from_utf8_lossy(&header[..16])
            .trim_end()
            .trim_end_matches('/')
            .to_owned();
        let size: usize = std::str::from_utf8(&header[48..58])
            .ok()
            .and_then(|size| size.trim().parse().ok())
            .ok_or_else(|| package("an ar header has no size"))?;
        if after.len() < size {
            return Err(package(format!("the ar member {name} is cut short")));
        }
        members.push((name, &after[..size]));
        // Members start on even offsets.
        let next = size + size % 2;
        rest = &after[next.min(after.len())..];
    }
    Ok(members)
}

/// Decompresses `bytes` by their magic number: gzip, xz, zstd (each frame,
/// as `zstd -T0` writes several), or none.
pub(crate) fn decompress(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    if bytes.starts_with(&[0x1f, 0x8b]) {
        flate2::read::MultiGzDecoder::new(bytes)
            .read_to_end(&mut out)
            .map_err(package)?;
    } else if bytes.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0]) {
        lzma_rs::xz_decompress(&mut std::io::BufReader::new(bytes), &mut out).map_err(package)?;
    } else if bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        let mut rest = bytes;
        while !rest.is_empty() {
            let mut decoder =
                ruzstd::decoding::StreamingDecoder::new_with_max_window_size(rest, 1 << 31)
                    .map_err(package)?;
            decoder.read_to_end(&mut out).map_err(package)?;
            rest = decoder.into_inner();
        }
    } else {
        out.extend_from_slice(bytes);
    }
    Ok(out)
}

/// One entry of a tar archive.
pub(crate) struct Entry {
    /// Its path, without a leading `./`.
    pub path: String,
    /// Whether it is a folder.
    pub folder: bool,
    /// Its contents, for the files `keep` asked for.
    pub contents: Option<Vec<u8>>,
}

/// The entries of a tar archive, with the contents of those `keep` accepts.
pub(crate) fn tar_entries(tar: &[u8], keep: impl Fn(&str) -> bool) -> Result<Vec<Entry>> {
    let mut archive = tar::Archive::new(tar);
    let mut entries = Vec::new();
    for entry in archive.entries().map_err(package)? {
        let mut entry = entry.map_err(package)?;
        let path = entry
            .path()
            .map_err(package)?
            .to_string_lossy()
            .into_owned();
        let path = path.trim_start_matches("./").to_owned();
        if path.is_empty() {
            continue;
        }
        let folder = entry.header().entry_type().is_dir();
        let contents = if !folder && keep(&path) {
            let mut contents = Vec::new();
            entry.read_to_end(&mut contents).map_err(package)?;
            Some(contents)
        } else {
            None
        };
        entries.push(Entry {
            path: path.trim_end_matches('/').to_owned(),
            folder,
            contents,
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn ar_members_are_read_with_their_padding() {
        let mut ar = b"!<arch>\n".to_vec();
        for (name, data) in [("debian-binary", &b"2.0\n"[..]), ("odd", b"abc")] {
            ar.extend_from_slice(
                format!(
                    "{name:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
                    0,
                    0,
                    0,
                    100644,
                    data.len()
                )
                .as_bytes(),
            );
            ar.extend_from_slice(data);
            if data.len() % 2 == 1 {
                ar.push(b'\n');
            }
        }
        let members = ar_members(&ar).unwrap();
        assert_eq!(members[0], ("debian-binary".to_owned(), &b"2.0\n"[..]));
        assert_eq!(members[1], ("odd".to_owned(), &b"abc"[..]));
        assert!(ar_members(b"not ar").is_err());
    }

    #[test]
    fn gzip_and_plain_bytes_decompress() {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(b"hello").unwrap();
        assert_eq!(decompress(&gz.finish().unwrap()).unwrap(), b"hello");
        assert_eq!(decompress(b"plain").unwrap(), b"plain");
    }

    #[test]
    fn zstd_frames_one_after_another_decompress() {
        let frame = |data: &[u8]| {
            ruzstd::encoding::compress_to_vec(data, ruzstd::encoding::CompressionLevel::Fastest)
        };
        let mut both = frame(b"first ");
        both.extend(frame(b"second"));
        assert_eq!(decompress(&both).unwrap(), b"first second");
    }
}
