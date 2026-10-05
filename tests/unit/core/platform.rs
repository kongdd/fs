use super::*;
use std::path::Path;

#[test]
fn stamp_match_ignores_only_windows_attribute_word() {
    let stored = 7u64.to_le_bytes().repeat(4);
    let mut current = stored.clone();
    current[24] = 0xff;
    #[cfg(windows)]
    assert!(stamp_matches(&stored, &current));
    #[cfg(not(windows))]
    assert!(!stamp_matches(&stored, &current));
    current[8] = 1;
    assert!(!stamp_matches(&stored, &current));
}

#[test]
fn path_prefix_matches_components_not_string_prefixes() {
    assert!(path_starts_with(
        Path::new("/data/db"),
        Path::new("/data/db")
    ));
    assert!(path_starts_with(
        Path::new("/data/db/extra"),
        Path::new("/data/db")
    ));
    assert!(!path_starts_with(
        Path::new("/data/db-journal"),
        Path::new("/data/db")
    ));
    assert!(path_starts_with(Path::new("/data"), Path::new("/")));
    assert!(same_path(Path::new("/data/db"), Path::new("/data/db")));
}

#[cfg(windows)]
#[test]
fn windows_path_prefix_ignores_slash_case_and_verbatim() {
    assert!(path_starts_with(
        Path::new(r"C:\Users\fs\c.db"),
        Path::new("c:/Users/fs/c.db")
    ));
    assert!(path_starts_with(
        Path::new(r"\\?\C:\Users\fs\c.db"),
        Path::new("c:/users/fs/c.db")
    ));
    assert!(same_path(
        Path::new(r"C:\Users\fs"),
        Path::new("c:/Users/fs")
    ));
    assert!(!path_starts_with(
        Path::new(r"C:\Users\fs\c.db-journal"),
        Path::new("c:/Users/fs/c.db")
    ));
}

#[test]
fn portable_prefix_handles_drive_case_and_windows_separators() {
    for path in [
        b"C:/data".as_slice(),
        b"C:/data/item.nc",
        br"C:\data\item.nc",
    ] {
        assert!(portable_prefix(path, b"c:/data"));
    }
    assert!(!portable_prefix(b"C:/data-other", b"c:/data"));
    assert!(!portable_prefix(b"C:/data", b"d:/data"));
    assert!(portable_prefix(
        br"\\?\UNC\nas\share\item.nc",
        b"//nas/share"
    ));
}

#[test]
fn portable_prefix_preserves_unix_backslashes() {
    assert!(!portable_prefix(br"/data/cache\file.nc", b"/data/cache"));
    assert!(portable_prefix(br"/data/a\b/item.nc", br"/data/a\b"));
    assert!(!portable_prefix(b"/data/a/b/item.nc", br"/data/a\b"));
}

#[test]
fn portable_keys_borrow_already_normalized_paths() {
    for path in [
        b"/data/item/".as_slice(),
        b"c:/data",
        b"//nas/share",
        br"/data/a\b",
    ] {
        assert!(matches!(slash_key(path), Cow::Borrowed(_)));
    }
    for path in [b"C:/data".as_slice(), br"c:\data", b"//?/UNC/nas/share"] {
        assert!(matches!(slash_key(path), Cow::Owned(_)));
    }
    for (path, expected) in [
        (b"/".as_slice(), b"/".as_slice()),
        (b"c:////", b"c:/"),
        (b"/data///", b"/data"),
        (b"//?/UNC/nas/share/", b"//nas/share"),
        (b"//?/C:/data/", b"c:/data"),
        (b"C:relative", b"c:relative"),
    ] {
        assert_eq!(slash_key(path).as_ref(), expected);
    }
    assert!(!portable_prefix(b"/data", b""));
    assert!(portable_prefix(b"c:/data", b"C:/"));
}

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
        .as_chunks::<8>()
        .0
        .iter()
        .map(|&bytes| u64::from_le_bytes(bytes))
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
