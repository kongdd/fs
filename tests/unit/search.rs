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
fn extension_filter_is_case_insensitive() {
    let a = idx("a", "/data", &["tmp", ".pyc"]);
    assert!(is_excluded(b"/data/x.TMP", &[&a]));
    assert!(is_excluded(b"/data/x.pyc", &[&a]));
    assert!(!is_excluded(b"/data/x.nc", &[&a]));
}

#[test]
fn selection_uses_component_boundaries_and_literal_paths() {
    let extensions = vec![".NC".into(), "tif".into()];
    let root = Some(Path::new("/data/project[*]"));
    assert!(matches_selection(
        b"/data/project[*]/a.nc",
        root,
        &extensions
    ));
    assert!(!matches_selection(
        b"/data/project[*]-other/a.nc",
        root,
        &extensions
    ));
    assert!(!matches_selection(
        b"/data/project[*]/a.txt",
        root,
        &extensions
    ));
    assert!(matches_selection(
        b"/data/project[*]/bad_\xff.NC",
        root,
        &extensions
    ));
    assert!(!matches_selection(
        b"/data/project[*]/no_extension",
        root,
        &extensions
    ));
    assert!(matches_selection(
        b"/data/project[*]/no_extension",
        root,
        &[]
    ));
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
fn nested_root_uses_longest_match() {
    let outer = idx("outer", "/data", &["tmp"]);
    let inner = idx("inner", "/data/keep", &[]);
    assert!(!is_excluded(b"/data/keep/file.tmp", &[&outer, &inner]));
}
