use std::path::PathBuf;

use super::*;

fn idx(name: &str, root: &str, exts: &[&str]) -> IndexConfig {
    IndexConfig {
        name: name.into(),
        root: PathBuf::from(root),
        database: PathBuf::from(format!("/tmp/{name}.db")),
        update_database: None,
        search_database: None,
        filters: crate::config::Filters {
            exclude_extensions: exts.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        },
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
fn scopes_preserve_portable_roots() {
    for (input, expected) in [
        ("//nas/share/./project/../data", "//nas/share/data"),
        (r"\\nas\share\project\..\data", "//nas/share/data"),
        (r"C:\project\..\data", "C:/data"),
    ] {
        let scope = resolve_scope(Path::new(input)).unwrap();
        assert_eq!(scope, PathBuf::from(expected));
        assert!(matches_selection(expected.as_bytes(), Some(&scope), &[]).unwrap());
        let child = format!("{expected}/file.nc");
        assert!(matches_selection(child.as_bytes(), Some(&scope), &[]).unwrap());
    }
}

#[cfg(unix)]
#[test]
fn unix_backslash_names_are_not_descendants_or_excluded() {
    let path = br"/data/cache\file.nc";
    assert!(!matches_selection(path, Some(Path::new("/data/cache")), &[]).unwrap());
    let mut index = idx("test", "/data", &[]);
    index.filters.exclude_paths.push("/data/cache".into());
    assert!(!is_excluded(path, &[&index]).unwrap());
    let scope = resolve_scope(Path::new(r"/data/a\b")).unwrap();
    assert_eq!(scope, PathBuf::from(r"/data/a\b"));
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
fn compiled_exclusions_resolve_paths_once_and_preserve_ownership() {
    let mut outer = idx("outer", "/data", &["tmp"]);
    outer.filters.exclude_paths = vec!["cache".into(), "/elsewhere".into()];
    outer.filters.exclude_files = vec!["SECRET".into()];
    let inner = idx("inner", "/data/cache/keep", &[]);
    let filters = Exclusions::new(&[&outer, &inner]);
    assert_eq!(
        filters.indexes[1].1,
        vec![PathBuf::from("/data/cache"), PathBuf::from("/elsewhere")]
    );
    for path in [
        b"/data/cache/x".as_slice(),
        b"/data/file.TMP",
        b"/data/secret",
    ] {
        assert!(filters.matches(path).unwrap());
    }
    for path in [
        b"/data/cache-other/x".as_slice(),
        b"/data/cache/keep/file.tmp",
        b"/outside/secret",
        b"/data/bad_\xff.nc",
    ] {
        assert!(!filters.matches(path).unwrap());
    }
    assert!(Exclusions::new(&[&inner]).indexes.is_empty());
}

#[test]
fn empty_filters_take_the_fast_path() {
    let path = b"/data/bad_\xff.nc";
    assert!(matches_selection(path, None, &[]).unwrap());
    assert!(matches_kind(path, false, false).unwrap());
    assert!(!is_ignored_dir(path, &[]).unwrap());
    assert!(!Exclusions::new(&[]).matches(path).unwrap());
}

#[test]
fn nested_root_uses_longest_match() {
    let outer = idx("outer", "/data", &["tmp"]);
    let inner = idx("inner", "/data/keep", &[]);
    assert!(!is_excluded(b"/data/keep/file.tmp", &[&outer, &inner]).unwrap());
}
