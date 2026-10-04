use super::*;

#[test]
fn native_strings_roundtrip() {
    let text = OsString::from("中文🌧.nc");
    assert_eq!(os_string(text.as_encoded_bytes().to_vec()).unwrap(), text);
    let path = std::env::temp_dir().join(text);
    assert_eq!(path_from_bytes(&path_bytes(&path)).unwrap(), path);
}

#[test]
fn directory_stamp_keeps_identity_and_whole_seconds() {
    let metadata = std::fs::metadata(std::env::temp_dir()).unwrap();
    #[cfg(unix)]
    let expected = {
        use std::os::unix::fs::MetadataExt;
        [
            metadata.dev(),
            metadata.ino(),
            metadata.mtime() as u64,
            metadata.ctime() as u64,
        ]
    };
    #[cfg(windows)]
    let expected = {
        use std::os::windows::fs::MetadataExt;
        [
            metadata.creation_time() / 10_000_000,
            metadata.last_write_time() / 10_000_000,
            metadata.file_size(),
            u64::from(metadata.file_attributes()),
        ]
    };
    let stamp = directory_stamp(&metadata);
    assert_eq!(stamp.len(), 32);
    let actual: Vec<u64> = stamp
        .chunks_exact(8)
        .map(|bytes| u64::from_le_bytes(bytes.try_into().unwrap()))
        .collect();
    assert_eq!(actual, expected);
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
