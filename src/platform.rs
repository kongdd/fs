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

pub fn directory_stamp(metadata: &Metadata) -> Vec<u8> {
    #[cfg(unix)]
    let values = {
        use std::os::unix::fs::MetadataExt;
        [
            metadata.dev(),
            metadata.ino(),
            metadata.mtime() as u64,
            metadata.mtime_nsec() as u64,
            metadata.ctime() as u64,
            metadata.ctime_nsec() as u64,
        ]
    };
    #[cfg(windows)]
    let values = {
        use std::os::windows::fs::MetadataExt;
        [
            metadata.creation_time(),
            metadata.last_write_time(),
            metadata.file_size(),
            u64::from(metadata.file_attributes()),
        ]
    };
    values.into_iter().flat_map(u64::to_le_bytes).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_strings_roundtrip() {
        let text = OsString::from("中文🌧.nc");
        assert_eq!(os_string(text.as_encoded_bytes().to_vec()).unwrap(), text);
        let path = std::env::temp_dir().join(text);
        assert_eq!(path_from_bytes(&path_bytes(&path)).unwrap(), path);
    }

    #[cfg(windows)]
    #[test]
    fn windows_surrogates_roundtrip_and_corruption_is_rejected() {
        use std::os::windows::ffi::OsStringExt;
        let text = OsString::from_wide(&[b'a' as u16, 0xd800, b'b' as u16, 0xdcff]);
        assert_eq!(os_string(text.as_encoded_bytes().to_vec()).unwrap(), text);
        for bytes in [
            vec![0xff],
            vec![0xed, 0xa0],
            vec![0xc0, 0x80],
            vec![0xed, 0xa0, 0x80, 0xed, 0xb0, 0x80],
        ] {
            assert!(os_string(bytes).is_err());
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_roots_and_verbatim_paths_roundtrip() {
        for text in [
            r"C:\",
            r"\\server\share",
            r"\\?\C:\data\中文",
            r"\\?\UNC\server\share\data",
        ] {
            let path = Path::new(text);
            assert_eq!(path_from_bytes(&path_bytes(path)).unwrap(), path);
        }
    }
}
