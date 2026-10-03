use super::*;
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;

fn query(pattern: OsString, basename: bool, ignore_case: bool) -> Query {
    Query::new(&SearchOptions {
        patterns: vec![pattern],
        basename,
        ignore_case,
        ..Default::default()
    })
    .unwrap()
}

#[test]
fn literal_case_and_path_scope() {
    assert!(query("soil".into(), true, true).matches(b"/data/SOIL.nc"));
    assert!(!query("soil".into(), true, false).matches(b"/data/SOIL.nc"));
    assert!(!query("soil".into(), true, true).matches(b"/soil/other.nc"));
    assert!(query("soil".into(), false, true).matches(b"/soil/other.nc"));
}

#[test]
fn globs_match_full_names_and_preserve_raw_bytes() {
    assert!(query("*.nc".into(), true, true).matches(b"/data/bad_\xff.NC"));
    assert!(query("*.txt".into(), true, false).matches(b"/data/line\nfile.txt"));
    assert!(query("[sr]oil?.nc".into(), true, false).matches(b"/data/soil1.nc"));
    assert!(!query("soil*.nc".into(), true, false).matches(b"/data/xsoil.nc"));
    #[cfg(unix)]
    assert!(
        query(OsString::from_vec(b"bad_\xff".to_vec()), true, false).matches(b"/data/bad_\xff.nc")
    );
}

#[test]
fn regex_and_invalid_patterns() {
    let q = Query::new(&SearchOptions {
        patterns: vec!["^soil[0-9]+[.]nc$".into()],
        basename: true,
        ignore_case: true,
        regex: true,
        ..Default::default()
    })
    .unwrap();
    assert!(q.matches(b"/data/SOIL12.nc"));
    assert!(!q.matches(b"/data/soil.nc"));
    for pattern in ["[", "[abc", "[]"] {
        assert!(
            Query::new(&SearchOptions {
                patterns: vec![pattern.into()],
                ..Default::default()
            })
            .is_err()
        );
    }
    assert!(
        Query::new(&SearchOptions {
            patterns: vec!["(".into()],
            regex: true,
            ..Default::default()
        })
        .is_err()
    );
}
