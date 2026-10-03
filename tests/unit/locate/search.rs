use std::path::PathBuf;

use super::*;

fn idx(name: &str, root: &str, exts: &[&str]) -> IndexConfig {
    IndexConfig {
        name: name.into(),
        root: PathBuf::from(root),
        database: PathBuf::from(format!("/tmp/{name}.db")),
        filters: crate::config::Filters {
            exclude_extensions: exts.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        },
    }
}

#[test]
fn mount_mapping_is_byte_safe_and_respects_component_boundaries() {
    for (path, expected) in [
        (b"/volume1/CMIP6".as_slice(), b"/mnt/z".as_slice()),
        (b"/volume1/CMIP6/project/a.nc", b"/mnt/z/project/a.nc"),
        (b"/volume2/GitHub/repo/README", b"/mnt/x/repo/README"),
        (b"/volume2/GitHub", b"/mnt/x"),
        (b"/volume1/Researches", b"/mnt/y"),
        (b"/volume1/Researches/project/a.nc", b"/mnt/y/project/a.nc"),
        (b"/volume1/CUG-hydro/", b"/mnt/o/"),
        (b"/volume1/CUG-hydro/project/a.nc", b"/mnt/o/project/a.nc"),
        (b"/volume1/CMIP6/bad_\xff\n.nc", b"/mnt/z/bad_\xff\n.nc"),
    ] {
        assert_eq!(map_mount_path(path).unwrap(), expected);
    }
    for path in [
        b"/volume1/CMIP6-other/a.nc".as_slice(),
        b"/volume2/GitHubBackup/a.nc",
        b"/volume1/Researches-other/a.nc",
        b"/volume1/CUG-hydro-other/a.nc",
        b"/other/volume1/CMIP6/a.nc",
        b"/mnt/z/a.nc",
        b"/volume1/cmip6/a.nc",
    ] {
        assert!(map_mount_path(path).is_none());
    }
}

#[test]
fn directory_name_ignore_matches_at_any_depth_with_exact_components() {
    let names = vec!["cache".into(), "node_modules".into(), ".cache".into()];
    for path in [
        b"/data/cache".as_slice(),
        b"/data/cache/file.nc",
        b"/other/nested/cache/file.nc",
        b"/data/node_modules/a.js",
        b"/data/.cache/bad_\xff.nc",
    ] {
        assert!(is_ignored_dir(path, &names).unwrap());
    }
    for path in [
        b"/data/cache-other/file.nc".as_slice(),
        b"/data/CACHE/file.nc",
        b"/data/mycache",
        b"/data/cache.nc",
        b"/data/keep/file.nc",
    ] {
        assert!(!is_ignored_dir(path, &names).unwrap());
    }
    assert!(!is_ignored_dir(b"/data/cache/file.nc", &[]).unwrap());
}

#[test]
fn kind_filter_uses_only_the_filename_suffix() {
    for path in [
        b"/data.v1/README".as_slice(),
        b"/data/folder",
        b"/data/.gitignore",
        b"/data/name.",
    ] {
        assert!(matches_kind(path, true, false).unwrap());
        assert!(!matches_kind(path, false, true).unwrap());
        assert!(matches_kind(path, false, false).unwrap());
    }
    for path in [
        b"/data/a.nc".as_slice(),
        b"/data/archive.tar.gz",
        b"/data/.config.json",
        b"/data/folder.v1",
        b"/data/bad_\xff.nc",
    ] {
        assert!(!matches_kind(path, true, false).unwrap());
        assert!(matches_kind(path, false, true).unwrap());
        assert!(matches_kind(path, false, false).unwrap());
    }
}

#[test]
fn extension_filter_is_case_insensitive() {
    let a = idx("a", "/data", &["tmp", ".pyc"]);
    assert!(is_excluded(b"/data/x.TMP", &[&a]).unwrap());
    assert!(is_excluded(b"/data/x.pyc", &[&a]).unwrap());
    assert!(!is_excluded(b"/data/x.nc", &[&a]).unwrap());
}

#[test]
fn selection_uses_component_boundaries_and_literal_paths() {
    let extensions = vec![".NC".into(), "tif".into()];
    let root = Some(Path::new("/data/project[*]"));
    assert!(matches_selection(b"/data/project[*]/a.nc", root, &extensions).unwrap());
    assert!(!matches_selection(b"/data/project[*]-other/a.nc", root, &extensions).unwrap());
    assert!(!matches_selection(b"/data/project[*]/a.txt", root, &extensions).unwrap());
    assert!(matches_selection(b"/data/project[*]/bad_\xff.NC", root, &extensions).unwrap());
    assert!(!matches_selection(b"/data/project[*]/no_extension", root, &extensions).unwrap());
    assert!(matches_selection(b"/data/project[*]/no_extension", root, &[]).unwrap());
}

#[test]
fn scope_normalizes_without_accessing_the_directory() {
    assert_eq!(
        resolve_scope(Path::new("/missing/./sub/../project")).unwrap(),
        PathBuf::from("/missing/project")
    );
    assert_eq!(
        resolve_scope(Path::new(".")).unwrap(),
        std::env::current_dir().unwrap()
    );
}

#[test]
fn ignored_paths_use_component_boundaries_and_cover_descendants() {
    let mut outer = idx("outer", "/data", &[]);
    let mut inner = idx("inner", "/data/cache/nested", &[]);
    for index in [&mut outer, &mut inner] {
        index.filters.exclude_paths.push("/data/cache".into());
    }
    for path in [
        b"/data/cache".as_slice(),
        b"/data/cache/file.nc",
        b"/data/cache/nested/bad_\xff.nc",
    ] {
        assert!(is_excluded(path, &[&outer, &inner]).unwrap());
    }
    assert!(!is_excluded(b"/data/cache-other/file.nc", &[&outer, &inner]).unwrap());
    assert!(!is_excluded(b"/data/CACHE/file.nc", &[&outer, &inner]).unwrap());
}

#[test]
fn nested_root_uses_longest_match() {
    let outer = idx("outer", "/data", &["tmp"]);
    let inner = idx("inner", "/data/keep", &[]);
    assert!(!is_excluded(b"/data/keep/file.tmp", &[&outer, &inner]).unwrap());
}
