//! Lossless native strings; indexed Windows paths use forward slashes.
use std::{
    borrow::Cow,
    ffi::OsString,
    fs::Metadata,
    path::{Path, PathBuf},
};

use anyhow::Result;

pub fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
    #[cfg(unix)]
    let home = std::env::var_os("HOME");
    home.map(PathBuf::from)
}

pub fn normalize(bytes: &[u8]) -> Cow<'_, [u8]> {
    #[cfg(windows)]
    if bytes.contains(&b'\\') {
        return Cow::Owned(
            bytes
                .iter()
                .map(|&b| if b == b'\\' { b'/' } else { b })
                .collect(),
        );
    }
    Cow::Borrowed(bytes)
}

/// Component prefix match. Windows ignores slash style, ASCII case, and `\\?\`.
pub fn path_starts_with(path: &Path, prefix: &Path) -> bool {
    let path = path_key(path);
    let prefix = path_key(prefix);
    if prefix.is_empty() {
        return false;
    }
    if path == prefix {
        return true;
    }
    let prefix = prefix.strip_suffix(b"/").unwrap_or(prefix.as_slice());
    if prefix == b"/" {
        return path.starts_with(b"/");
    }
    path.strip_prefix(prefix)
        .is_some_and(|rest| rest.starts_with(b"/"))
}

pub fn same_path(left: &Path, right: &Path) -> bool {
    path_key(left) == path_key(right)
}

fn path_key(path: &Path) -> Vec<u8> {
    let mut bytes = path.as_os_str().as_encoded_bytes().to_vec();
    #[cfg(windows)]
    {
        for byte in &mut bytes {
            if *byte == b'\\' {
                *byte = b'/';
            }
            *byte = byte.to_ascii_lowercase();
        }
        if let Some(rest) = bytes.strip_prefix(b"//?/unc/") {
            let mut unc = b"//".to_vec();
            unc.extend_from_slice(rest);
            bytes = unc;
        } else if let Some(rest) = bytes.strip_prefix(b"//?/") {
            bytes = rest.to_vec();
        }
    }
    while bytes.len() > 1 && bytes.last() == Some(&b'/') {
        let drive_root = bytes.len() == 3 && bytes[1] == b':';
        if drive_root {
            break;
        }
        bytes.pop();
    }
    bytes
}

/// `C:/...` and `//server/share` are absolute on every OS so a Windows index can be queried elsewhere.
pub fn portable_absolute(path: &Path) -> bool {
    path.is_absolute() || is_drive_or_unc(path.as_os_str().as_encoded_bytes())
}

/// Config and index paths use `/`, including Windows drive roots (`C:` -> `C:/`).
pub fn portable_path(path: PathBuf) -> PathBuf {
    let raw = path.as_os_str().as_encoded_bytes();
    let windows_style = cfg!(windows) || is_drive_or_unc(raw) || raw.starts_with(br"\\");
    if !windows_style || (!raw.contains(&b'\\') && raw.len() != 2) {
        return path;
    }
    let mut bytes = raw.to_vec();
    if bytes.len() == 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        bytes.push(b'/');
    }
    for byte in &mut bytes {
        if *byte == b'\\' {
            *byte = b'/';
        }
    }
    os_string(bytes).map(PathBuf::from).unwrap_or(path)
}

/// Prefix match on stored `/` paths. Drive letters ignore ASCII case; NAS paths stay case-sensitive.
pub fn portable_prefix(path: &[u8], prefix: &[u8]) -> bool {
    let path = slash_key(path);
    let prefix = slash_key(prefix);
    if prefix.is_empty() {
        return false;
    }
    if path == prefix {
        return true;
    }
    let prefix = prefix.strip_suffix(b"/").unwrap_or(prefix.as_ref());
    if prefix == b"/" {
        return path.starts_with(b"/");
    }
    path.strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(b"/"))
}

fn slash_key(bytes: &[u8]) -> Cow<'_, [u8]> {
    let windows_style = is_drive_or_unc(bytes);
    let uppercase_drive = bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_uppercase();
    if !uppercase_drive
        && (!windows_style || (!bytes.contains(&b'\\') && !bytes.starts_with(b"//?/")))
    {
        return Cow::Borrowed(trim_slashes(bytes));
    }
    let mut bytes = bytes
        .iter()
        .map(|&byte| {
            if windows_style && byte == b'\\' {
                b'/'
            } else {
                byte
            }
        })
        .collect::<Vec<_>>();
    if bytes
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"//?/unc/"))
    {
        let mut unc = b"//".to_vec();
        unc.extend_from_slice(&bytes[8..]);
        bytes = unc;
    } else if let Some(rest) = bytes.strip_prefix(b"//?/") {
        bytes = rest.to_vec();
    }
    if bytes.len() >= 2 && bytes[1] == b':' {
        bytes[0] = bytes[0].to_ascii_lowercase();
    }
    bytes.truncate(trim_slashes(&bytes).len());
    Cow::Owned(bytes)
}

fn trim_slashes(mut bytes: &[u8]) -> &[u8] {
    while bytes.len() > 1 && bytes.last() == Some(&b'/') {
        if bytes.len() == 3 && bytes[1] == b':' {
            break;
        }
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

fn is_drive_or_unc(bytes: &[u8]) -> bool {
    (bytes.len() >= 2
        && bytes[1] == b':'
        && bytes[0].is_ascii_alphabetic()
        && (bytes.len() == 2 || matches!(bytes[2], b'/' | b'\\')))
        || bytes.starts_with(b"//")
        || bytes.starts_with(br"\\")
}

pub fn path_bytes(path: &Path) -> Cow<'_, [u8]> {
    let bytes = normalize(path.as_os_str().as_encoded_bytes());
    #[cfg(windows)]
    if path.is_absolute() && path.parent().is_none() && !bytes.ends_with(b"/") {
        let mut bytes = bytes.into_owned();
        bytes.push(b'/');
        return Cow::Owned(bytes);
    }
    bytes
}

pub fn os_string(bytes: Vec<u8>) -> Result<OsString> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(OsString::from_vec(bytes))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        // Decode WTF-8 without trusting bytes read from a database. UTF-8
        // handles ordinary text; isolated UTF-16 surrogates need three bytes.
        let mut rest = bytes.as_slice();
        let mut wide = Vec::new();
        loop {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    wide.extend(text.encode_utf16());
                    break;
                }
                Err(error) => {
                    let (text, tail) = rest.split_at(error.valid_up_to());
                    wide.extend(std::str::from_utf8(text)?.encode_utf16());
                    if tail.len() < 3
                        || tail[0] != 0xed
                        || !(0xa0..=0xbf).contains(&tail[1])
                        || !(0x80..=0xbf).contains(&tail[2])
                    {
                        anyhow::bail!("invalid Windows filename encoding");
                    }
                    wide.push(
                        0xd000 | (u16::from(tail[1] & 0x3f) << 6) | u16::from(tail[2] & 0x3f),
                    );
                    rest = &tail[3..];
                }
            }
        }
        let string = OsString::from_wide(&wide);
        if string.as_encoded_bytes() != bytes {
            anyhow::bail!("noncanonical Windows filename encoding");
        }
        Ok(string)
    }
}

pub fn path_from_bytes(bytes: &[u8]) -> Result<Cow<'_, Path>> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(Cow::Borrowed(Path::new(std::ffi::OsStr::from_bytes(bytes))))
    }
    #[cfg(windows)]
    {
        let bytes = bytes
            .iter()
            .map(|&b| if b == b'/' { b'\\' } else { b })
            .collect();
        Ok(Cow::Owned(os_string(bytes)?.into()))
    }
}

pub fn file_identity(metadata: &Metadata) -> (i64, i64, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (metadata.mtime(), metadata.mtime_nsec(), metadata.ino())
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        (
            metadata.last_write_time() as i64,
            metadata.creation_time() as i64,
            u64::from(metadata.file_attributes()),
        )
    }
}

/// Directory identity/change marker with whole-second timestamps.
/// Changes within the same second can be missed, including during a scan.
pub fn directory_stamp(metadata: &Metadata) -> Vec<u8> {
    #[cfg(unix)]
    let values = {
        use std::os::unix::fs::MetadataExt;
        [
            metadata.dev(),
            metadata.ino(),
            metadata.mtime() as u64,
            metadata.ctime() as u64,
        ]
    };
    #[cfg(windows)]
    let values = {
        use std::os::windows::fs::MetadataExt;
        [
            metadata.creation_time() / 10_000_000,
            metadata.last_write_time() / 10_000_000,
            metadata.file_size(),
            u64::from(metadata.file_attributes()),
        ]
    };
    values.into_iter().flat_map(u64::to_le_bytes).collect()
}

/// Whether a stored directory stamp still matches a fresh stat.
/// Windows ignores attribute bits: archive, indexed, and temporary flags flip
/// without a listing change and would otherwise rescan the whole tree.
pub fn stamp_matches(stored: &[u8], current: &[u8]) -> bool {
    #[cfg(windows)]
    {
        stored.len() == current.len() && stored.len() >= 24 && stored[..24] == current[..24]
    }
    #[cfg(not(windows))]
    {
        stored == current
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/core/platform.rs"]
mod tests;
