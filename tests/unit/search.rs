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
fn nested_root_uses_longest_match() {
    let outer = idx("outer", "/data", &["tmp"]);
    let inner = idx("inner", "/data/keep", &[]);
    assert!(!is_excluded(b"/data/keep/file.tmp", &[&outer, &inner]));
}
